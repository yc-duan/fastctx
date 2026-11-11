//! Window edges keep complete stretches whenever a stretch fits the budget.

use crate::timeline_window::Window;
use fastctx_types::session::{Happened, SessionEvent, What};

pub(crate) fn boundary(event: Option<&SessionEvent>) -> bool {
    event.is_some_and(|event| {
        matches!(
            &event.happened,
            Happened::TurnStarted { .. }
                | Happened::Heard { .. }
                | Happened::Finished { item: fastctx_types::session::Item { what: What::Message { .. }, .. }, .. }
        )
    })
}

fn member(event: Option<&SessionEvent>) -> bool {
    event.is_some_and(|event| match &event.happened {
        Happened::Finished { item, .. } | Happened::Began { item, .. } | Happened::Updated { item, .. } => {
            matches!(item.what, What::Tool(_) | What::Thinking { .. } | What::AgentActivity { .. })
        }
        _ => false,
    })
}

pub(crate) fn align(window: &mut Window) {
    if window.older
        && !window.left_complete
        && !window.oversized_left
        && !boundary(window.entries.first().and_then(|entry| entry.event.as_ref()))
        && let Some(at) = window.entries.iter().position(|entry| boundary(entry.event.as_ref()))
    {
        window.entries.drain(..at);
        window.interval_start = window.entries.first().map(|entry| entry.place.clone());
        window.left_complete = true;
    }
    if window.newer
        && !window.right_complete
        && !window.oversized_right
        && !boundary(window.entries.last().and_then(|entry| entry.event.as_ref()))
        && let Some(at) = window.entries.iter().rposition(|entry| boundary(entry.event.as_ref()))
        && let Some(first) = window.entries[at + 1..].iter().position(|entry| member(entry.event.as_ref()))
    {
        let end = at + 1 + first;
        window.right = window.entries.get(end).map(|entry| entry.place.clone());
        window.entries.truncate(end);
        window.interval_end = window.entries.last().map(|entry| entry.place.clone());
    }
    window.partial_older = window.older
        && !window.left_complete
        && !boundary(window.entries.first().and_then(|entry| entry.event.as_ref()));
    let after = window.entries.iter().rposition(|entry| boundary(entry.event.as_ref())).map_or(0, |at| at + 1);
    window.partial_newer = window.newer
        && !window.right_complete
        && window.entries[after..].iter().any(|entry| member(entry.event.as_ref()));
}
