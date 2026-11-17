//! Reading a conversation's timeline: a page of its events, whether it has
//! moved, what its records brought since a reading, and the bytes of an
//! event kept whole as an object. A page is one bounded range of the
//! timeline's index, whatever the history's length; its changes are the
//! records written since, however much the timeline holds.

use crate::Node;
use fastctx_content::State;
pub(crate) use fastctx_content::{Anchor, Place};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use std::time::Duration;

/// The most a page holds, whatever is asked.
pub(crate) use fastctx_client::timeline_copy::{PAGE_BYTES, PAGE_ITEMS};

pub(crate) use fastctx_client::timeline_copy::{Change, Changed, Entry, Read, likeness, shown};

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Whether the conversation's timeline is read from the asset library
/// (`local`), not the content timeline.
pub(crate) fn from_assets(node: &Node, resident: &ResidentId) -> Result<bool, Failure> {
    Ok(super::local::held(node, resident)?.is_some())
}

/// A completed reading always installs a checkpoint, even without a host stamp.
pub(crate) fn initializing(node: &Node, resident: &ResidentId) -> Result<bool, Failure> {
    if from_assets(node, resident)? {
        return Ok(false);
    }
    node.content()?.checkpoint(&super::checkpoint(resident.as_str())).map(|state| state.is_none()).map_err(failure)
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
    if let Some(held) = super::local::held(node, resident)? {
        return super::local::page(&held, anchor, max_items.clamp(1, PAGE_ITEMS), max_bytes.min(PAGE_BYTES));
    }
    let page = node
        .content()?
        .page(&super::stream(resident), anchor, max_items.clamp(1, PAGE_ITEMS), max_bytes.min(PAGE_BYTES))
        .map_err(failure)?;
    let entries = page
        .items
        .into_iter()
        .map(|item| {
            Ok(Entry {
                place: item.place(),
                sequence: item.sequence,
                shown: shown(&item.body, &item.objects)?,
                likeness: likeness(&item.body),
            })
        })
        .collect::<Result<_, Failure>>()?;
    Ok(Read { revision: page.revision, entries, older: page.older, newer: page.newer, through: page.through })
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
    if let Some(held) = super::local::held(node, resident)? {
        return super::local::changes(&held, after, max_items.clamp(1, PAGE_ITEMS), max_bytes.min(PAGE_BYTES));
    }
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
                    likeness: likeness(&body),
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
    let until = tokio::time::Instant::now() + within;
    let mut waited = false;
    loop {
        if let Some(held) = super::local::held(node, resident)? {
            // Made in the asset library while this waited on the content
            // timeline: its revision is news to the reader, who reads it there.
            if waited {
                return super::local::moved(&held, 0, Duration::ZERO).await;
            }
            return super::local::moved(&held, after, until.saturating_duration_since(tokio::time::Instant::now())).await;
        }
        let left = until.saturating_duration_since(tokio::time::Instant::now());
        // While its asset timeline is being made, the content timeline is not
        // (`api::here`), and the making wakes nothing here: it is looked for
        // again each moment, and every few while it may begin.
        let every = if super::tail::producing(resident) { Duration::from_millis(500) } else { Duration::from_secs(2) };
        let slice = left.min(every);
        let moved = node.content()?.moved(&super::stream(resident), after, slice).await.map_err(failure)?;
        if moved.is_some() || slice >= left {
            return Ok(moved);
        }
        waited = true;
    }
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
    if let Some(held) = super::local::held(node, resident)?
        && let Some(bytes) = super::local::object(&held, hash, offset, length)?
    {
        return Ok(Some(bytes));
    }
    let content = node.content()?;
    if !content.refers(&super::stream(resident).scope, hash).map_err(failure)? {
        return Ok(None);
    }
    content.objects().read(hash, offset, length).map_err(failure)
}
