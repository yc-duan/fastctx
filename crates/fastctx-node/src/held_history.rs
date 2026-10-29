//! A watch for a viewer that keeps the history on its own disk
//! (`fastctx_types::session::held`; `world-gui.md`, 对话). This machine keeps
//! nothing for it: it reads the history again, slims it to what an interface
//! draws, cuts it into segments and sends the ones the viewer lacks. Where
//! the records the history is read from are as they were when the viewer
//! last read it, it says so without reading them whole.

use std::sync::Arc;

use fastctx_net::history::{self, Pictures};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::Activity;
use fastctx_types::session::held::Held;
use fastctx_types::session::{SessionEvent, Watched};
use fastctx_types::{Address, ResidentId};
use sha2::{Digest as _, Sha256};

use crate::Node;
use crate::talk::Opening;

/// Named in every version, so that a viewer's copy made by an older way of
/// slimming or cutting is never taken for the history as it is sent now.
const FORM: &str = "fastctx history segments: turn boundaries, artifact identities and shared output text";

/// The first pieces of a watch of `resident` for a viewer that holds
/// `held`, and the events after them, slimmed.
pub(crate) async fn opening(node: &Arc<Node>, resident: &ResidentId, held: &Held) -> Result<Opening, Failure> {
    let owner = Address { machine: node.machine(), resident: resident.clone() };
    // Subscribe before checking a held version: a rewrite between the check
    // and subscription must still reach the viewer that gets "unchanged".
    let subscribed = held.version.as_ref().map(|_| crate::talk::subscribe(node, resident)).transpose()?;
    let version = stamp(node, resident).await?;
    if let Some(version) = &version
        && held.version.as_ref() == Some(version)
    {
        let (events, pending) = subscribed.expect("a held version subscribed before its stamp");
        let mut pieces = vec![Watched::Unchanged { version: version.clone() }];
        pieces.extend(live(pending, &owner));
        return Ok(Opening { pieces, events, slim: Some(owner) });
    }
    drop(subscribed);
    let read = crate::talk::history_here(node, resident).await?;
    // Never repeated: a viewer that holds it is sent the history next time.
    // So is a reading some record was missing from, which is read again
    // once it can be.
    let version = match version {
        Some(version) if !read.unread => version,
        _ => format!("unstamped {:x}", u128::from_le_bytes(random()?)),
    };
    let (history, held_now, slimmed_for) = (read.history, held.clone(), owner.clone());
    let mut history = tokio::task::spawn_blocking(move || {
        let pictures = Pictures::Referenced { owner: &slimmed_for };
        let mut history = history;
        history.retain_mut(|event| history::slim(event, &pictures));
        history
    })
    .await
    .map_err(|error| Failure::new(format!("cutting the history into segments: {error}")))?;
    picture_fingerprints(node, &mut history).await?;
    let mut pieces = tokio::task::spawn_blocking(move || history::pieces(history::cut(history), &held_now, version))
        .await
        .map_err(|error| Failure::new(format!("cutting the history into segments: {error}")))?;
    pieces.extend(live(read.pending, &owner));
    Ok(Opening { pieces, events: read.events, slim: Some(owner) })
}

pub(crate) async fn picture_fingerprints(node: &Arc<Node>, events: &mut [SessionEvent]) -> Result<(), Failure> {
    for event in events {
        let fastctx_types::session::Happened::Heard { message: Some(message), .. } = &mut event.happened else {
            continue;
        };
        for image in &mut message.images {
            if image.content.is_some() {
                continue;
            }
            let file = match crate::attachment_files::resolve(
                node,
                image.source.clone(),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .await
            {
                Ok(file) => file,
                Err(error) if error.code == Some(fastctx_types::ipc::FailureCode::OriginalGone) => continue,
                Err(error) => return Err(error),
            };
            image.content = Some(
                tokio::task::spawn_blocking(move || fastctx_hands::files::content_fingerprint(&file.path))
                    .await
                    .map_err(|error| Failure::new(error.to_string()))?
                    .map_err(|error| Failure::new(error.to_string()))?,
            );
        }
    }
    Ok(())
}

/// What the resident's host waits on someone to answer now, sent as it
/// happens rather than kept with the history, which outlives it.
fn live(pending: Vec<SessionEvent>, owner: &Address) -> impl Iterator<Item = Watched> + '_ {
    pending.into_iter().filter_map(|event| crate::talk::live(event, Some(owner)).map(|event| Watched::Live { event }))
}

/// A version of the resident's history, from what it is read from: equal
/// versions read the same history. None while a turn runs, when its host
/// adds to its record without saying so to any of these.
async fn stamp(node: &Arc<Node>, resident: &ResidentId) -> Result<Option<String>, Failure> {
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    if node.residents.now(resident).is_none_or(|now| now.turn.is_some() || now.activity == Activity::Busy) {
        return Ok(None);
    }
    let mut digest = Sha256::new();
    let mut take = |bytes: &[u8]| {
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    };
    take(FORM.as_bytes());
    take(env!("CARGO_PKG_VERSION").as_bytes());
    take(&serde_json::to_vec(&about).map_err(|error| Failure::new(error.to_string()))?);
    let failed = |error: std::io::Error| Failure::new(format!("reading {resident}'s records: {error}"));
    {
        let directory = node.world.conversation(resident);
        let _transaction = crate::resident_storage::lock();
        crate::session_record::stamp(directory.path(), &mut take).map_err(failed)?;
        crate::run_facts::stamp(&directory, &mut take).map_err(failed)?;
    }
    match (about.host.agent.as_str(), &about.session) {
        // A thread the user opened in the Codex command line runs turns the
        // World does not see, so its record can grow under a stamp that
        // would not change.
        ("codex", Some(_)) if !about.world_managed() => return Ok(None),
        ("codex", Some(thread)) => {
            let codex = node.codex.of(node, &about)?;
            take(codex.stamp(thread).await.map_err(Failure::new)?.as_bytes());
        }
        ("claude", _) => crate::claude::history::stamp(node, resident, &mut take),
        _ => {}
    }
    Ok(Some(digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect()))
}

fn random() -> Result<[u8; 16], Failure> {
    fastctx_net::sign::random::<16>().map_err(|error| Failure::new(format!("naming a version: {error}")))
}

/// The host's own record of one item of a resident's conversation on any
/// machine.
pub(crate) async fn item_detail(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<fastctx_types::access::Written>,
    turn: Option<String>,
    item: String,
) -> Result<(Address, Option<serde_json::Value>), Failure> {
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.about(&resident).is_some() => node.machine().into(),
        None => crate::colleagues::find_for(node, &fastctx_types::access::Acting::Here, &resident).await?,
    };
    if local || node.is_here(&machine) {
        let detail = item_detail_here(node, &resident, turn.as_deref(), &item).await?;
        return Ok((Address { machine: machine.name, resident }, detail));
    }
    let call = fastctx_types::remote::RemoteCall::ItemDetail { resident: resident.clone(), turn, item };
    let written = &machine;
    match node
        .call(&fastctx_types::access::Acting::Here, written, fastctx_types::access::Purpose::Agents, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))?
    {
        Ok(fastctx_types::remote::RemoteReply::ItemDetail { detail }) => {
            Ok((Address { machine: machine.name, resident }, detail))
        }
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than an item's record"))),
        Err(failure) => Err(failure),
    }
}

/// The host's own record of one item of a resident of this machine, read
/// out of its whole history as it is now.
pub(crate) async fn item_detail_here(
    node: &Arc<Node>,
    resident: &ResidentId,
    turn: Option<&str>,
    item: &str,
) -> Result<Option<serde_json::Value>, Failure> {
    use fastctx_types::session::Happened;
    let read = crate::talk::history_here(node, resident).await?;
    let found = read.history.into_iter().rev().find_map(|event| match event.happened {
        Happened::Finished { turn: at, item: found } | Happened::Updated { turn: at, item: found }
            if turn.is_none_or(|turn| turn == at) && found.id == item =>
        {
            Some(found.detail.unwrap_or(serde_json::Value::Null))
        }
        Happened::Heard { message: Some(message), turn: at, .. }
            if turn.is_none_or(|turn| at.as_deref() == Some(turn)) && message.id == item =>
        {
            Some(message.detail)
        }
        _ => None,
    });
    Ok(found)
}
