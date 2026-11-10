//! Reading a conversation's timeline: a page of its events, whether it has
//! moved, what its records brought since a reading, and the bytes of an
//! event kept whole as an object. A page is one bounded range of the
//! timeline's index, whatever the history's length; its changes are the
//! records written since, however much the timeline holds.

use super::build::{EVENT, OVERSIZED};
use crate::Node;
use fastctx_content::State;
pub(crate) use fastctx_content::{Anchor, ObjectRef, Place};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use std::time::Duration;

/// The most a page holds, whatever is asked.
pub(crate) const PAGE_ITEMS: u32 = 400;
pub(crate) const PAGE_BYTES: u32 = 1024 * 1024;

pub(crate) enum Shown {
    /// The event, as JSON.
    Event(Vec<u8>),
    /// An event too large for a page: its length as JSON, what is shown in
    /// its place when something is (JSON), and the object that holds it whole.
    Oversized { bytes: u64, slim: Option<Vec<u8>>, object: ObjectRef },
}

pub(crate) struct Entry {
    pub place: Place,
    /// The timeline's count of records as this one was written.
    pub sequence: u64,
    pub shown: Shown,
}

pub(crate) struct Read {
    /// The timeline's revision as the page was read; none while the
    /// conversation's timeline has not been made.
    pub revision: Option<u64>,
    pub entries: Vec<Entry>,
    pub older: Option<Place>,
    pub newer: Option<Place>,
    /// How many records the timeline held as the page was read.
    pub through: u64,
}

pub(crate) enum Change {
    /// The event stands at its place as shown from this record on.
    Is(Entry),
    /// The timeline holds the item no more.
    Gone { item: Vec<u8> },
}

pub(crate) struct Changed {
    pub revision: Option<u64>,
    /// In the order they were written.
    pub changes: Vec<Change>,
    /// The count of records these reach to.
    pub through: u64,
    pub more: bool,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// A page of the conversation's timeline. It blocks for the read: on a task
/// thread it waits in place.
pub(crate) fn page(
    node: &Node,
    resident: &ResidentId,
    anchor: &Anchor,
    max_items: u32,
    max_bytes: u32,
) -> Result<Read, Failure> {
    let page = node
        .content()?
        .page(&super::stream(resident), anchor, max_items.clamp(1, PAGE_ITEMS), max_bytes.min(PAGE_BYTES))
        .map_err(failure)?;
    let entries = page
        .items
        .into_iter()
        .map(|item| {
            Ok(Entry { place: item.place(), sequence: item.sequence, shown: shown(&item.body, &item.objects)? })
        })
        .collect::<Result<_, Failure>>()?;
    Ok(Read { revision: page.revision, entries, older: page.older, newer: page.newer, through: page.through })
}

/// What a timeline record's body shows: the event, or what stands for one
/// kept whole as an object.
fn shown(body: &[u8], objects: &[ObjectRef]) -> Result<Shown, Failure> {
    match body.split_first() {
        Some((&EVENT, event)) => Ok(Shown::Event(event.to_vec())),
        Some((&OVERSIZED, told)) => {
            let told: serde_json::Value = serde_json::from_slice(told).map_err(failure)?;
            let object = *objects.first().ok_or_else(|| failure("an oversized event names no object"))?;
            let slim = match &told["slim"] {
                serde_json::Value::Null => None,
                slim => Some(serde_json::to_vec(slim).map_err(failure)?),
            };
            Ok(Shown::Oversized { bytes: told["bytes"].as_u64().unwrap_or(object.size), slim, object })
        }
        _ => Err(failure("the timeline holds an item it cannot read")),
    }
}

/// What the timeline's records brought after its first `after`: none when
/// it does not hold its records from there on, and the reader reads its
/// pages again. It blocks for the read: on a task thread it waits in place.
pub(crate) fn changes(
    node: &Node,
    resident: &ResidentId,
    after: u64,
    max_items: u32,
    max_bytes: u32,
) -> Result<Option<Changed>, Failure> {
    let read = node
        .content()?
        .changes(&super::stream(resident), after, max_items.clamp(1, PAGE_ITEMS), max_bytes.min(PAGE_BYTES))
        .map_err(failure)?;
    let Some(read) = read else { return Ok(None) };
    let changes = read
        .changes
        .into_iter()
        .map(|change| {
            let record = change.record;
            Ok(match record.state {
                State::Is { body, objects } => Change::Is(Entry {
                    shown: shown(&body, &objects)?,
                    place: Place { order: record.order, item: record.item },
                    sequence: change.sequence,
                }),
                State::Gone => Change::Gone { item: record.item },
            })
        })
        .collect::<Result<_, Failure>>()?;
    Ok(Some(Changed { revision: read.revision, changes, through: read.through, more: read.more }))
}

/// Waits until the conversation's timeline is past `after`, `within` at
/// most, and answers its revision then.
pub(crate) async fn moved(
    node: &Node,
    resident: &ResidentId,
    after: u64,
    within: Duration,
) -> Result<Option<u64>, Failure> {
    node.content()?.moved(&super::stream(resident), after, within).await.map_err(failure)
}

/// A range of an object the conversation's timeline refers to; none when it
/// refers to no such object.
pub(crate) fn object(
    node: &Node,
    resident: &ResidentId,
    hash: &[u8; 32],
    offset: u64,
    length: u32,
) -> Result<Option<Vec<u8>>, Failure> {
    let content = node.content()?;
    if !content.refers(&super::stream(resident).scope, hash).map_err(failure)? {
        return Ok(None);
    }
    content.objects().read(hash, offset, length).map_err(failure)
}
