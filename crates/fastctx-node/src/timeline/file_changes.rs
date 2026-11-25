//! The files a conversation changed, kept beside its timeline (`gui-files`,
//! `conversation.files`): each change of one file a host's edit made, and
//! each file one of the World's runs wrote, one record named as the page
//! names it — `i:{turn}:{item}#{index}` for the index-th change of an edit,
//! `w:{write}` for a write. Made from the timeline as it is written here,
//! from where the last look stopped (a checkpoint of the library), so a
//! conversation that grows is read only where it grew; the timeline gives a
//! run's facts again in place as they change, and so do these. An edit that
//! failed or was declined changed nothing: its records go. A change's lines
//! ride along while they are short; longer ones are read where they are
//! kept. Looked at by the timeline's job after each building.

use super::assets::{EVENT_OBJECT_BYTES, object_whole};
use super::beside::{Beside, writable};
use crate::Node;
use fastctx_content::v2::{
    ConversationFileChange, EVENT, Entry, Kind as _, Library, OVERSIZED, Timeline, ViewChange, Write, Writer,
};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::{ChangeHow, Does, Happened, SessionEvent, ToolStatus, What};
use fastctx_types::write::ChangeKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The most timeline records one read takes, and their bytes; the most reads
/// one look makes, and the most changes it writes — the rest in the next
/// (OO23).
const RECORDS: u32 = 64;
const READ_BYTES: u32 = 4 << 20;
const READS: usize = 32;
const WRITES_A_LOOK: usize = 4096;
/// The most bytes of changes one read gathers before they are written: the
/// records after wait for the next read.
const FOUND_BYTES: usize = 8 << 20;
/// The most looks that wait for an event's object to come before what
/// stands for it in its record is taken instead.
const AWAITED_LOOKS: u32 = 5;
/// The most changes one commit writes.
const AT_ONCE: usize = 256;
/// The most a change's record takes with its lines; past it, without them.
const BODY_BYTES: usize = 15 * 1024;
/// The longest name a change has (`ConversationFileChange`).
const NAME_BYTES: usize = 256;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Where the last look stopped in the timeline, in which holding period,
/// and how many looks stopped before one record (`awaited_at`) waiting for
/// its event's object.
#[derive(Serialize, Deserialize)]
struct Made {
    holding: i64,
    through: u64,
    #[serde(default)]
    awaited: u32,
    #[serde(default)]
    awaited_at: u64,
}

fn checkpoint(scope: &str) -> String {
    format!("files-made/{scope}")
}

/// The first revision of the timeline as it stands now: its tag, nothing
/// read of it.
fn start_of(revision: u64) -> u64 {
    revision & !((1u64 << 40) - 1)
}

/// What every ended item's record says of it (`Happened::Finished`).
const FINISHED: &[u8] = b"\"finished\"";

/// Whether `event` is a call that ended: the only kind that changes files.
fn ended_call(event: &SessionEvent) -> bool {
    matches!(&event.happened, Happened::Finished { item, .. } if matches!(item.what, What::Tool(_)))
}

/// What a record of the timeline tells of files changed.
enum Told {
    /// A call that ended.
    Ended(SessionEvent),
    /// Nothing of files.
    Nothing,
    /// A call kept whole in an object not here yet, with what stands for it
    /// in its record, which may leave changes out.
    Awaited(Option<SessionEvent>),
}

/// An ended call as told, nothing otherwise.
fn told(event: Option<SessionEvent>) -> Told {
    event.filter(ended_call).map_or(Told::Nothing, Told::Ended)
}

/// What the record `entry` of the timeline tells. One kept whole in an
/// object is read whole only when what stands for it in its record says it
/// is an ended call; one past what is read whole, or whose bytes are no
/// event, is what stands for it; one whose object cannot be read now fails
/// the look, tried again. A record that cannot be read tells nothing, never
/// stopping the reading of those after it.
fn event_in(library: &Library, entry: &Entry) -> Result<Told, Failure> {
    match entry.body.split_first() {
        // Most events are no ended call: those are not read whole.
        Some((&EVENT, encoded)) if !encoded.windows(FINISHED.len()).any(|window| window == FINISHED) => {
            Ok(Told::Nothing)
        }
        Some((&EVENT, encoded)) => Ok(told(serde_json::from_slice(encoded).ok())),
        Some((&OVERSIZED, record)) => {
            let slim = serde_json::from_slice::<serde_json::Value>(record).ok().and_then(|record| {
                serde_json::from_value::<Option<SessionEvent>>(record["slim"].clone()).ok().flatten()
            });
            if slim.as_ref().is_some_and(|slim| !ended_call(slim)) {
                return Ok(Told::Nothing);
            }
            let Some(object) = entry.objects.first() else { return Ok(told(slim)) };
            let awaited = || Told::Awaited(slim.clone().filter(ended_call));
            match library.size(&object.hash).map_err(failure)? {
                None => return Ok(awaited()),
                Some(size) if size > EVENT_OBJECT_BYTES => return Ok(told(slim)),
                Some(_) => {}
            }
            let Some(bytes) = object_whole(library, &object.hash)? else { return Ok(awaited()) };
            Ok(match serde_json::from_slice::<SessionEvent>(&bytes) {
                Ok(event) => told(Some(event)),
                Err(_) => told(slim),
            })
        }
        _ => Ok(Told::Nothing),
    }
}

/// How many lines a host's diff adds and removes.
fn counted(diff: &str) -> (u64, u64) {
    let (mut added, mut removed) = (0, 0);
    for line in diff.lines() {
        if line.starts_with("+++ ") || line.starts_with("--- ") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

/// `body` as it is written, where it fits a record.
fn fitted(body: &serde_json::Value) -> Option<Vec<u8>> {
    serde_json::to_vec(body).ok().filter(|bytes| bytes.len() <= BODY_BYTES)
}

/// `body` with `lines` as its `diff`, where the whole fits a record.
fn with_lines(body: &serde_json::Value, lines: serde_json::Value) -> Option<Vec<u8>> {
    let mut body = body.clone();
    body["diff"] = lines;
    fitted(&body)
}

/// The changes `event` tells of, each by its name: its record, or none where
/// it is to go (an edit that changed nothing after all); the bytes of the
/// records it gathered.
fn changes_of(event: &SessionEvent, into: &mut BTreeMap<String, Option<Vec<u8>>>) -> usize {
    let Happened::Finished { turn, item } = &event.happened else { return 0 };
    let What::Tool(tool) = &item.what else { return 0 };
    let at = event.at.as_millisecond();
    let undone = matches!(tool.status, ToolStatus::Failed | ToolStatus::Declined);
    let mut gathered = 0;
    if let Does::Edit { changes } = &tool.does {
        for (index, change) in changes.iter().enumerate() {
            let name = format!("i:{turn}:{}#{index}", item.id);
            if name.len() > NAME_BYTES {
                continue;
            }
            if undone || change.path.is_empty() {
                into.insert(name, None);
                continue;
            }
            let how = match change.how {
                ChangeHow::Added => "added",
                ChangeHow::Deleted => "removed",
                _ => "changed",
            };
            let (added, removed) = counted(&change.diff);
            let body = serde_json::json!({
                "source": "edit",
                "at": at,
                "turn": turn,
                "call": item.id,
                "machine": tool.machine,
                "path": change.path,
                "how": how,
                "added": added,
                "removed": removed,
            });
            let lines = serde_json::Value::String(change.diff.clone());
            if let Some(bytes) = with_lines(&body, lines).or_else(|| fitted(&body)) {
                gathered += bytes.len();
                into.insert(name, Some(bytes));
            }
        }
    }
    for write in tool.run.as_ref().map(|run| run.writes.as_slice()).unwrap_or_default() {
        let name = format!("w:{}", write.id);
        if name.len() > NAME_BYTES || write.path.is_empty() {
            continue;
        }
        let how = match write.kind {
            ChangeKind::Added => "added",
            ChangeKind::Modified => "changed",
            ChangeKind::Deleted => "removed",
        };
        let body = serde_json::json!({
            "source": "write",
            "at": at,
            "turn": turn,
            "call": item.id,
            "machine": write.machine,
            "path": write.path,
            "how": how,
            "added": write.added,
            "removed": write.removed,
        });
        // Where its lines are kept, with the lines themselves while short.
        let bytes = match &write.diff {
            Some(diff) => {
                let mut bare = diff.clone();
                bare.lines = None;
                with_lines(&body, serde_json::to_value(diff).unwrap_or_default())
                    .or_else(|| with_lines(&body, serde_json::to_value(&bare).unwrap_or_default()))
                    .or_else(|| fitted(&body))
            }
            None => fitted(&body),
        };
        if let Some(bytes) = bytes {
            gathered += bytes.len();
            into.insert(name, Some(bytes));
        }
    }
    gathered
}

/// Writes of `found` what differs from what is kept; how many.
fn commit(
    library: &Library,
    scope: &str,
    writer: &Writer<ConversationFileChange>,
    found: BTreeMap<String, Option<Vec<u8>>>,
    signer: &Arc<dyn fastctx_content::v2::Signer>,
) -> Result<usize, Failure> {
    let mut writes = Vec::new();
    for (item, body) in found {
        let item = item.into_bytes();
        let kept = library.item(scope, ConversationFileChange::STREAM, &item).map_err(failure)?;
        match (&body, &kept) {
            (None, None) => continue,
            (Some(body), Some(kept)) if kept.body == *body => continue,
            _ => {}
        }
        writes.push(Write::<ConversationFileChange> { order: item.clone(), item, value: body, objects: Vec::new() });
    }
    let written = writes.len();
    let mut writes = writes.into_iter().peekable();
    while writes.peek().is_some() {
        let batch: Vec<_> = writes.by_ref().take(AT_ONCE).collect();
        writer.write_all(batch, Vec::new(), false, signer.clone()).map_err(failure)?;
    }
    Ok(written)
}

/// Brings `resident`'s file changes up to its timeline as it is written
/// here: those of what was written since the last look; how many written.
pub(super) async fn produce(
    node: &Arc<Node>,
    resident: &ResidentId,
    producing: &super::serial::Serial,
) -> Result<usize, Failure> {
    let held = producing.hold(resident).await;
    let Some(Beside { library, scope, holding }) =
        writable(node, resident, ConversationFileChange::STREAM, &held).await?
    else {
        return Ok(0);
    };
    let signer = node.assets.as_ref().map(|assets| assets.signer()).ok_or_else(|| failure("no asset library"))?;
    let (written, more, waiting) = fastctx_platform::task::blocking(move || {
        let _held = held;
        let name = checkpoint(&scope);
        let Some(revision) = library.view_revision(&scope, Timeline::STREAM).map_err(failure)? else {
            return Ok::<_, Failure>((0, false, false));
        };
        let made = library
            .checkpoint(&name)
            .map_err(failure)?
            .and_then(|bytes| serde_json::from_slice::<Made>(&bytes).ok())
            .filter(|made| made.holding == holding && start_of(made.through) == start_of(revision));
        let (mut awaited, mut awaited_at) = made.as_ref().map_or((0, 0), |made| (made.awaited, made.awaited_at));
        let mut from = made.map_or(start_of(revision), |made| made.through);
        if from == revision {
            return Ok((0, false, false));
        }
        let writer = Writer::<ConversationFileChange>::new(&library, &scope, ConversationFileChange::STREAM)
            .map_err(failure)?
            .in_holding(holding);
        let (mut written, mut more, mut waiting) = (0, true, false);
        for _ in 0..READS {
            // Made anew meanwhile: the next look starts it again from its
            // start as it stands then, its tag no longer the one kept.
            let Some(read) =
                library.view_changes(&scope, Timeline::STREAM, from, RECORDS, READ_BYTES).map_err(failure)?
            else {
                break;
            };
            // Read through here; one stopped before a record reads it
            // again from there: a mark before it, in the timeline's tag.
            let (mut through, mut cut) = (read.through, false);
            let before = |entry: &Entry| start_of(read.through) | entry.local.saturating_sub(1);
            let (mut found, mut gathered) = (BTreeMap::new(), 0);
            for change in &read.changes {
                let ViewChange::Is(entry) = change else { continue };
                let event = match event_in(&library, entry)? {
                    Told::Nothing => continue,
                    Told::Ended(event) => event,
                    // Waited for long enough: what stands for it.
                    Told::Awaited(slim) if awaited_at == before(entry) && awaited >= AWAITED_LOOKS => match slim {
                        Some(slim) => slim,
                        None => continue,
                    },
                    Told::Awaited(_) => {
                        let here = before(entry);
                        awaited = if awaited_at == here { awaited + 1 } else { 1 };
                        (awaited_at, through, cut, waiting) = (here, here, true, true);
                        break;
                    }
                };
                if gathered >= FOUND_BYTES || written + found.len() >= WRITES_A_LOOK {
                    (through, cut) = (before(entry), true);
                    break;
                }
                gathered += changes_of(&event, &mut found);
            }
            written += commit(&library, &scope, &writer, found, &signer)?;
            from = through;
            let state = serde_json::to_vec(&Made { holding, through: from, awaited, awaited_at }).map_err(failure)?;
            library.keep_checkpoint(&name, state).map_err(failure)?;
            // Waiting for an object: the look fails, and the job comes
            // again soon.
            if waiting {
                more = false;
                break;
            }
            if !cut && !read.more {
                more = false;
                break;
            }
            if written >= WRITES_A_LOOK {
                break;
            }
        }
        Ok((written, more, waiting))
    })
    .await?;
    if waiting {
        return Err(failure(format!("{written} written; an event's object is not here yet, looked at again soon")));
    }
    // Cut short by its bounds: the rest in the next look.
    if more {
        super::tail::wanted(node, resident);
    }
    Ok(written)
}
