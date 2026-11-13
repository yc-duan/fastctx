//! A conversation's timeline read from the asset library (design §28.3,
//! plan 3-5): once the library holds the conversation's timeline — made
//! here, or come made from elsewhere — a reader reads it there, at once,
//! however long the history; before that, and for a conversation on no
//! network, from the content timeline (`reads`). Reads only read: nothing
//! is produced for them.

use super::reads::{Change, Changed, Entry, Read, likeness, shown};
use crate::Node;
use fastctx_content::v2::{Kind as _, Library, Timeline, ViewChange, view_sequence};
use fastctx_content::{Anchor, Place};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use std::time::Duration;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Where a conversation's timeline is read in the asset library.
pub(crate) struct Held {
    library: Library,
    scope: String,
}

/// The conversation's timeline in the asset library, when a reader reads it
/// there: its scope held here, and its timeline made — by this machine's
/// producer at least once, or come from the device that orders it.
pub(crate) fn held(node: &Node, resident: &ResidentId) -> Result<Option<Held>, Failure> {
    let Some(kept) = node.assets.as_ref() else { return Ok(None) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(None) };
    let Some(placed) = super::assets::placed(node, &about)? else { return Ok(None) };
    let library = kept.library.clone();
    let Some(at) = library.view_revision(&placed.scope, Timeline::STREAM).map_err(failure)? else { return Ok(None) };
    // Made here only as the producer last finished making it whole in the
    // holding period the scope is in now: one of an earlier period, or
    // set back as a whole making began, is not (JJ05).
    let holding = library.holding(&placed.scope).map_err(failure)?;
    let made_here = library
        .checkpoint(&super::assets::checkpoint(resident))
        .map_err(failure)?
        .is_some_and(|state| holding.is_some() && super::tail::made_in(&state) == holding);
    let made_elsewhere = at & ((1 << 40) - 1) > 0
        && library
            .orderer(&placed.scope, Timeline::STREAM)
            .map_err(failure)?
            .is_some_and(|orderer| orderer != kept.device());
    Ok((made_here || made_elsewhere).then_some(Held { library, scope: placed.scope }))
}

/// An entry as a reader at `at` knows it: its sequence is its revision in
/// the era read in, so an entry made anew at the same revision is told
/// again (JJ04).
fn entry(at: u64, entry: fastctx_content::v2::Entry) -> Result<Entry, Failure> {
    Ok(Entry {
        shown: shown(&entry.body, &entry.objects)?,
        likeness: likeness(&entry.body),
        place: Place { order: entry.order, item: entry.item },
        sequence: view_sequence(at, entry.revision),
    })
}

/// A page of the timeline as the asset library holds it.
pub(crate) fn page(held: &Held, anchor: &Anchor, max_items: u32, max_bytes: u32) -> Result<Read, Failure> {
    let page = held.library.view_page(&held.scope, Timeline::STREAM, anchor, max_items, max_bytes).map_err(failure)?;
    Ok(Read {
        revision: page.revision,
        entries: page.entries.into_iter().map(|found| entry(page.through, found)).collect::<Result<_, Failure>>()?,
        older: page.older,
        newer: page.newer,
        through: page.through,
    })
}

/// What the timeline changed to after `after`, as the asset library holds
/// it; none when the reader reads its pages again.
pub(crate) fn changes(held: &Held, after: u64, max_items: u32, max_bytes: u32) -> Result<Option<Changed>, Failure> {
    let Some(read) = held.library.view_changes(&held.scope, Timeline::STREAM, after, max_items, max_bytes).map_err(failure)?
    else {
        return Ok(None);
    };
    let changes = read
        .changes
        .into_iter()
        .map(|change| {
            Ok(match change {
                ViewChange::Is(found) => Change::Is(entry(read.through, found)?),
                ViewChange::Gone { item } => Change::Gone { item },
            })
        })
        .collect::<Result<_, Failure>>()?;
    Ok(Some(Changed { revision: read.revision, changes, through: read.through, more: read.more }))
}

/// Waits until the timeline in the asset library is past `after`.
pub(crate) async fn moved(held: &Held, after: u64, within: Duration) -> Result<Option<u64>, Failure> {
    held.library.view_moved(&held.scope, Timeline::STREAM, after, within).await.map_err(failure)
}

/// A range of an object the timeline refers to; none when it refers to no
/// such object or the object is not here yet.
pub(crate) fn object(held: &Held, hash: &[u8; 32], offset: u64, length: u32) -> Result<Option<Vec<u8>>, Failure> {
    if !held.library.refers(&held.scope, hash).map_err(failure)? || held.library.size(hash).map_err(failure)?.is_none() {
        return Ok(None);
    }
    held.library.read(hash, offset, length).map(Some).map_err(failure)
}
