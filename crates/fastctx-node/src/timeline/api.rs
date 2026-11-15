//! Timeline calls use the same routes and admission as watching a conversation.

use super::reads::{self, Anchor, Change};
use fastctx_client::timeline_copy::{decode, decode_place, encode_place, event, hex};
use crate::Node;
use base64::{Engine, engine::general_purpose::STANDARD};
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::timeline_pages::{
    OBJECT_BYTES, TimelineAsk, TimelineChange, TimelineFrom, TimelineTold,
};
use std::{sync::Arc, time::Duration};

/// Admission is repeated when the directory changes and before handing over
/// an answer, including while a revision wait is open.
pub(crate) async fn checked<T>(
    node: &Node,
    reading: impl Future<Output = Result<T, Failure>>,
    admit: impl Fn() -> Result<(), Failure>,
) -> Result<T, Failure> {
    let mut views = node.views.subscribe();
    admit()?;
    tokio::pin!(reading);
    loop {
        tokio::select! {
            result = &mut reading => {
                admit()?;
                return result;
            }
            changed = views.changed() => {
                changed.map_err(|_| Failure::new("the node stopped"))?;
                admit()?;
            }
        }
    }
}

pub(crate) async fn call(
    node: &Arc<Node>,
    machine: Option<Written>,
    request: TimelineAsk,
) -> Result<TimelineTold, Failure> {
    let local = machine.is_none() && node.residents.about(request.resident()).is_some();
    // A place read from another machine, asked of this machine's library:
    // read anew (§28.30).
    let marked = super::copy::after(&request).is_some_and(|after| after & super::copy::ELSEWHERE != 0);
    if local || machine.as_ref().is_some_and(|machine| node.is_here(machine)) {
        return if marked { Ok(super::copy::anew(&request)) } else { here(node, request).await };
    }
    // Held here whole for one logged in here: read here, the machine it
    // lives on asked nothing. What it is running now only it has.
    if !matches!(request, TimelineAsk::Running { .. }) {
        let resident = request.resident().clone();
        let copy = fastctx_platform::roles::in_place("finding a conversation's copy", || {
            super::copy::of(node, &resident, machine.as_ref())
        })?;
        if let Some(held) = copy {
            return if marked { Ok(super::copy::anew(&request)) } else { from_copy(&held, request).await };
        }
    }
    let machine = match machine {
        Some(machine) => machine,
        None => crate::talk::locate(node, request.resident()).await?,
    };
    if node.is_here(&machine) {
        return if marked { Ok(super::copy::anew(&request)) } else { here(node, request).await };
    }
    // Asked of the machine it lives on, a place read here means nothing
    // there.
    if super::copy::after(&request).is_some_and(|after| after != 0 && !marked) {
        return Ok(super::copy::anew(&request));
    }
    let request = super::copy::unmarked(request);
    let mut refused = None;
    for (link, account) in node.speakers(&Acting::Here, &machine, Purpose::Agents).map_err(failure)? {
        let facts = link.call(&machine.name, &account, Purpose::Look, RemoteCall::Facts).await.map_err(failure)?;
        match facts {
            Ok(RemoteReply::Facts(facts)) if facts.timeline_reads => {}
            Err(error) if error.not_run => {
                refused = Some(error);
                continue;
            }
            Err(error) => return Err(error),
            _ => {
                refused = Some(Failure::not_run("this machine does not answer timeline pages; update it"));
                continue;
            }
        }
        let reply = fastctx_net::cancellable::call(
            &link,
            &machine.name,
            &account,
            RemoteCall::Timeline { request: request.clone() },
        )
        .await
        .map_err(failure)?;
        match reply {
            Ok(RemoteReply::Timeline(told)) => return Ok(super::copy::marked(told)),
            Err(error) if error.not_run => refused = Some(error),
            Err(error) => return Err(error),
            _ => return Err(Failure::new("the machine answered something other than a timeline reading")),
        }
    }
    Err(refused.expect("a speaker was tried"))
}

/// Reads another machine's conversation from this machine's copy (§28.30),
/// as a phone reads its replica (`fastctx_client::timeline_copy`).
async fn from_copy(held: &super::local::Held, request: TimelineAsk) -> Result<TimelineTold, Failure> {
    fastctx_client::timeline_copy::answer(held, request).await
}

/// Reads the timeline as it is kept. A reader of its end, or one waiting
/// for it to move, also asks for it to be brought to the conversation's
/// history, which is done after this reading and tells those who wait.
pub(crate) async fn here(node: &Arc<Node>, request: TimelineAsk) -> Result<TimelineTold, Failure> {
    let (about, _) =
        node.residents.about(request.resident()).ok_or_else(|| crate::forget::gone(node, request.resident()))?;
    if !about.world_managed() {
        return Err(Failure::refused(crate::speech::OWN_SESSION));
    }
    // Read from the content timeline, it is brought to the history as it
    // is read; the asset library's is made from its sources (§28.3). A lab
    // keeps the content timeline made as it is read all the same, to compare
    // the two by.
    // One whose asset timeline is being made now (a conversation not opened
    // since the node took the second version up) is read there in a moment:
    // its history is not read whole a second time beside it.
    if matches!(&request, TimelineAsk::Moved { .. } | TimelineAsk::Page { from: TimelineFrom::Latest, .. })
        && (std::env::var_os("FASTCTX_LAB_CONTENT_TIMELINE").is_some()
            || (!fastctx_platform::roles::in_place("choosing a timeline", || reads::from_assets(node, request.resident()))?
                && !super::tail::producing(request.resident())))
    {
        super::wanted_content(node, request.resident());
    }
    match request {
        TimelineAsk::Moved { resident, after } => {
            let revision = reads::moved(node, &resident, after, Duration::from_secs(25)).await?;
            let initializing = fastctx_platform::roles::in_place("reading timeline completion", || {
                reads::initializing(node, &resident)
            })?;
            Ok(TimelineTold::Moved { revision, initializing: Some(initializing) })
        }
        TimelineAsk::Running { resident, turn, item } => {
            fastctx_platform::roles::in_place("reading a running snapshot", || {
                let (event, mark) = node.watchers.running(&resident, &turn, &item);
                let event = event.filter(|event| fastctx_net::watch_queue::size(event) <= 8 << 20);
                Ok(TimelineTold::Running { event: event.map(Box::new), mark })
            })
        }
        TimelineAsk::Page { resident, from, max_events, max_bytes } => {
            let anchor = match from {
                TimelineFrom::Latest => Anchor::Latest,
                TimelineFrom::Before { place } => Anchor::Before(decode_place(place)?),
                TimelineFrom::After { place } => Anchor::After(decode_place(place)?),
            };
            fastctx_platform::roles::in_place("reading a timeline page", || {
                let initializing = reads::initializing(node, &resident)?;
                let read = reads::page(node, &resident, &anchor, max_events, max_bytes)?;
                Ok(TimelineTold::Page {
                    revision: read.revision,
                    initializing: Some(initializing),
                    events: read.entries.into_iter().map(event).collect::<Result<_, Failure>>()?,
                    older: read.older.map(encode_place),
                    newer: read.newer.map(encode_place),
                    through: Some(read.through),
                })
            })
        }
        TimelineAsk::Changes { resident, after, max_events, max_bytes } => {
            fastctx_platform::roles::in_place("reading a timeline's changes", || {
                let initializing = reads::initializing(node, &resident)?;
                let Some(read) = reads::changes(node, &resident, after, max_events, max_bytes)? else {
                    return Ok(TimelineTold::Reread);
                };
                let changes = read
                    .changes
                    .into_iter()
                    .map(|change| {
                        Ok(match change {
                            Change::Is(entry) => TimelineChange::Is { event: event(entry)? },
                            Change::Gone { item } => TimelineChange::Gone { item: hex(&item) },
                        })
                    })
                    .collect::<Result<_, Failure>>()?;
                Ok(TimelineTold::Changes {
                    revision: read.revision,
                    initializing: Some(initializing),
                    changes,
                    through: read.through,
                    more: read.more,
                })
            })
        }
        TimelineAsk::Object { resident, object, offset, length } => {
            let hash: [u8; 32] = decode(&object)?
                .try_into()
                .map_err(|_| Failure::new("a timeline object name must be a SHA-256 hash"))?;
            fastctx_platform::roles::in_place("reading a timeline object", || {
                let bytes = reads::object(node, &resident, &hash, offset, length.min(OBJECT_BYTES))?;
                Ok(TimelineTold::Object { base64: bytes.map(|bytes| STANDARD.encode(bytes)) })
            })
        }
    }
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
