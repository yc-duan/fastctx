//! A conversation of another machine read from this machine's copy of it
//! (plan 3-5, design §28.30). Once the copy is pulled whole in the holding
//! period it is in, and someone logged in here reads the conversation, a
//! reader reads the copy and asks the machine it lives on nothing: while
//! that machine is away, its conversations are read all the same. Reads
//! only read: nothing is fetched for them.
//!
//! A place read from another machine (a page's or changes' revision and
//! count, an event's sequence) carries [`ELSEWHERE`]; one read here never
//! does, its era having 24 bits. A place of one source asked of the other
//! is read anew, never taken for a number of the other's library.

use super::local::{self, Held};
use crate::Node;
use fastctx_content::v2::{Kind as _, Timeline};
use fastctx_types::ResidentId;
use fastctx_types::assets::{Holders, conversation_classes};
use fastctx_types::ipc::Failure;
use fastctx_types::timeline_pages::{TimelineAsk, TimelineChange, TimelineTold};

/// The mark of a place read from another machine.
pub(super) const ELSEWHERE: u64 = 1 << 63;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// This machine's copy of the conversation, when a reader here reads it:
/// held whole in its holding period, held as one logged in here reads it
/// (the hub lets it hold a class only those logged in hold), and its
/// timeline begun. One held only because this machine works for one of
/// its readers is read by no one here (§20, 3-3).
pub(crate) fn of(node: &Node, resident: &ResidentId) -> Result<Option<Held>, Failure> {
    let Some(kept) = node.assets.as_ref() else { return Ok(None) };
    let library = kept.library.clone();
    let logged: Vec<String> = conversation_classes()
        .into_iter()
        .filter(|class| class.holders == Holders::Logged)
        .map(|class| class.prefix)
        .collect();
    let mut found = None;
    for scope in library.scopes_ending(&format!("/{resident}")).map_err(failure)? {
        if !(scope.key.starts_with("c/") || scope.key.starts_with("b/")) || scope.pulled != Some(scope.holding) {
            continue;
        }
        let read_here = scope
            .allowed
            .as_deref()
            .is_some_and(|allowed| allowed.lines().any(|class| logged.iter().any(|logged| logged == class)));
        if !read_here {
            continue;
        }
        let Some(at) = library.view_revision(&scope.key, Timeline::STREAM).map_err(failure)? else { continue };
        if at & ((1 << 40) - 1) == 0 {
            continue;
        }
        // Ids are unique on their machine alone: two copies of that id, and
        // the one asked of is read where it lives.
        if found.replace(scope.key).is_some() {
            return Ok(None);
        }
    }
    Ok(found.map(|scope| local::Held::at(library, scope)))
}

/// Where a reader asks to read on from, when it does.
pub(super) fn after(request: &TimelineAsk) -> Option<u64> {
    match request {
        TimelineAsk::Moved { after, .. } | TimelineAsk::Changes { after, .. } => Some(*after),
        _ => None,
    }
}

/// The answer to a reader whose place is of another source: read anew.
pub(super) fn anew(request: &TimelineAsk) -> TimelineTold {
    match request {
        TimelineAsk::Moved { .. } => TimelineTold::Moved { revision: Some(0), initializing: None },
        _ => TimelineTold::Reread,
    }
}

/// The request as the machine it lives on is asked it: unmarked.
pub(super) fn unmarked(mut request: TimelineAsk) -> TimelineAsk {
    if let TimelineAsk::Moved { after, .. } | TimelineAsk::Changes { after, .. } = &mut request {
        *after &= !ELSEWHERE;
    }
    request
}

/// What the machine it lives on told, its places marked.
pub(super) fn marked(mut told: TimelineTold) -> TimelineTold {
    let mark = |place: &mut u64| *place |= ELSEWHERE;
    match &mut told {
        TimelineTold::Page { revision, events, through, .. } => {
            revision.iter_mut().for_each(mark);
            through.iter_mut().for_each(mark);
            events.iter_mut().for_each(|event| mark(&mut event.sequence));
        }
        TimelineTold::Changes { revision, changes, through, .. } => {
            revision.iter_mut().for_each(mark);
            mark(through);
            for change in changes {
                if let TimelineChange::Is { event } = change {
                    mark(&mut event.sequence);
                }
            }
        }
        TimelineTold::Moved { revision, .. } => revision.iter_mut().for_each(mark),
        TimelineTold::Running { .. } | TimelineTold::Object { .. } | TimelineTold::Reread => {}
    }
    told
}
