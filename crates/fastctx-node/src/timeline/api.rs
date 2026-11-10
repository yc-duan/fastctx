//! Timeline calls use the same routes and admission as watching a conversation.

use super::reads::{self, Anchor, Place, Shown};
use crate::Node;
use base64::{Engine, engine::general_purpose::STANDARD};
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::timeline_pages::{
    OBJECT_BYTES, TimelineAsk, TimelineEvent, TimelineFrom, TimelinePlace, TimelineShown, TimelineTold,
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
    let machine = match machine {
        Some(machine) => machine,
        None => crate::talk::locate(node, request.resident()).await?,
    };
    if local || node.is_here(&machine) {
        return here(node, request).await;
    }
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
            Ok(RemoteReply::Timeline(told)) => return Ok(told),
            Err(error) if error.not_run => refused = Some(error),
            Err(error) => return Err(error),
            _ => return Err(Failure::new("the machine answered something other than a timeline reading")),
        }
    }
    Err(refused.expect("a speaker was tried"))
}

/// Reads only the materialized index; an absent timeline stays absent.
pub(crate) async fn here(node: &Arc<Node>, request: TimelineAsk) -> Result<TimelineTold, Failure> {
    let (about, _) =
        node.residents.about(request.resident()).ok_or_else(|| crate::forget::gone(node, request.resident()))?;
    if !about.world_managed() {
        return Err(Failure::refused(crate::speech::OWN_SESSION));
    }
    match request {
        TimelineAsk::Moved { resident, after } => {
            let revision = reads::moved(node, &resident, after, Duration::from_secs(25)).await?;
            Ok(TimelineTold::Moved { revision })
        }
        TimelineAsk::Page { resident, from, max_events, max_bytes } => {
            let anchor = match from {
                TimelineFrom::Latest => Anchor::Latest,
                TimelineFrom::Before { place } => Anchor::Before(decode_place(place)?),
                TimelineFrom::After { place } => Anchor::After(decode_place(place)?),
            };
            fastctx_platform::roles::in_place("reading a timeline page", || {
                let read = reads::page(node, &resident, &anchor, max_events, max_bytes)?;
                let events = read
                    .entries
                    .into_iter()
                    .map(|entry| {
                        let shown = match entry.shown {
                            Shown::Event(event) => {
                                TimelineShown::Event { event: serde_json::from_slice(&event).map_err(failure)? }
                            }
                            Shown::Oversized { bytes, slim, object } => TimelineShown::Oversized {
                                bytes,
                                slim: slim.map(|slim| serde_json::from_slice(&slim)).transpose().map_err(failure)?,
                                object: hex(&object.hash),
                            },
                        };
                        Ok(TimelineEvent { place: encode_place(entry.place), sequence: entry.sequence, shown })
                    })
                    .collect::<Result<_, Failure>>()?;
                Ok(TimelineTold::Page {
                    revision: read.revision,
                    events,
                    older: read.older.map(encode_place),
                    newer: read.newer.map(encode_place),
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

fn decode_place(place: TimelinePlace) -> Result<Place, Failure> {
    let item = decode(&place.item)?;
    if item.is_empty() {
        return Err(Failure::new("a timeline item's identity is empty"));
    }
    Ok(Place { order: decode(&place.order)?, item })
}

fn encode_place(place: Place) -> TimelinePlace {
    TimelinePlace { order: hex(&place.order), item: hex(&place.item) }
}

fn decode(text: &str) -> Result<Vec<u8>, Failure> {
    let digit = |byte| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    if !text.len().is_multiple_of(2) || text.len() > fastctx_content::KEY_BYTES * 2 {
        return Err(Failure::new("a timeline position or object name has the wrong length"));
    }
    let mut bytes = vec![0; text.len() / 2];
    for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        let (Some(high), Some(low)) = (digit(pair[0]), digit(pair[1])) else {
            return Err(Failure::new("a timeline position or object name must be lowercase hexadecimal"));
        };
        *byte = high * 16 + low;
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}
