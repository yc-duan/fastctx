//! Apply one bounded batch of timeline records to the currently held interval.

use crate::timeline_window::{Window, covers};
use fastctx_types::timeline_pages::{TimelineChange, TimelinePlace};

fn compare(a: &TimelinePlace, b: &TimelinePlace) -> std::cmp::Ordering {
    (&a.order, &a.item).cmp(&(&b.order, &b.item))
}

pub(crate) fn apply(window: &mut Window, changes: Vec<TimelineChange>, following: bool) {
    let first = window.first();
    let last = window.last();
    let tail = following && !window.newer;
    for change in changes {
        match change {
            TimelineChange::Gone { item } => {
                let removed: Vec<_> =
                    window.entries.iter().filter(|e| e.place.item == item).filter_map(|e| e.event.clone()).collect();
                window.entries.retain(|e| e.place.item != item);
                if !removed.is_empty() {
                    window.retain_live(|live| {
                        crate::timeline_running::active(live) || !removed.iter().any(|old| covers(old, live))
                    });
                }
                if window.right.as_ref().is_some_and(|right| right.item == item) {
                    window.right = None;
                }
            }
            TimelineChange::Is { event } => {
                let before = first.as_ref().is_some_and(|first| event.place.order < first.order);
                let after = last.as_ref().is_some_and(|last| event.place.order > last.order);
                if (!before && !after) || (tail && after) || (tail && window.entries.is_empty()) {
                    window.merge(vec![event], false);
                } else {
                    // A held identity may have moved out of this interval.
                    let removed: Vec<_> = window
                        .entries
                        .iter()
                        .filter(|held| held.place.item == event.place.item)
                        .filter_map(|held| held.event.clone())
                        .collect();
                    window.entries.retain(|held| held.place.item != event.place.item);
                    if !removed.is_empty() {
                        window.retain_live(|live| {
                            crate::timeline_running::active(live) || !removed.iter().any(|old| covers(old, live))
                        });
                    }
                    if before {
                        window.older = true;
                    }
                    if after {
                        window.newer = true;
                        if window.right.as_ref().is_none_or(|right| compare(&event.place, right).is_lt()) {
                            window.right = Some(event.place);
                        }
                    }
                }
            }
        }
    }
}
