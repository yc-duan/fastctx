//! Resolve the membership of live identities that have no canonical event in
//! the staged window. Only the bounded current live set is retained; replayed
//! records are discarded after each page, including Is/Gone pairs in a batch.

use crate::timeline_window::{self, Window, covers};
use fastctx_client::World;
use fastctx_types::{
    ResidentId,
    access::Written,
    session::SessionEvent,
    timeline_pages::{PAGE_BYTES, PAGE_EVENTS, TimelineAsk, TimelineChange, TimelinePlace, TimelineTold},
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn candidates(current: &Window, staged: &Window) -> Vec<SessionEvent> {
    current
        .live
        .iter()
        .filter(|live| !staged.entries.iter().filter_map(|e| e.event.as_ref()).any(|event| covers(event, live)))
        .cloned()
        .collect()
}

pub(crate) async fn rejected(
    world: &World,
    machine: Option<Written>,
    resident: ResidentId,
    base: &Window,
    staged: &Window,
    candidates: &[SessionEvent],
    range: std::ops::Range<u64>,
) -> Result<BTreeSet<String>, String> {
    let (from, to) = (range.start, range.end);
    if from >= to || candidates.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut members: BTreeMap<String, Option<TimelinePlace>> = BTreeMap::new();
    for candidate in candidates {
        if let Some(entry) =
            base.entries.iter().find(|entry| entry.event.as_ref().is_some_and(|event| covers(event, candidate)))
        {
            members.insert(timeline_window::key(candidate), Some(entry.place.clone()));
        }
    }
    let mut after = from;
    while after < to {
        let reply = world
            .timeline(
                machine.clone(),
                TimelineAsk::Changes {
                    resident: resident.clone(),
                    after,
                    max_events: PAGE_EVENTS,
                    max_bytes: PAGE_BYTES,
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        let TimelineTold::Changes { changes, through, .. } = reply else {
            return Err("the timeline changed while live membership was being checked".into());
        };
        if through <= after || through - after != changes.len() as u64 {
            return Err("the timeline membership records did not advance".into());
        }
        for (offset, change) in changes.into_iter().enumerate() {
            if after + offset as u64 >= to {
                break;
            }
            match change {
                TimelineChange::Is { event } => {
                    if let Some(canonical) = timeline_window::decoded(&event) {
                        for candidate in candidates.iter().filter(|live| covers(&canonical, live)) {
                            members.insert(timeline_window::key(candidate), Some(event.place.clone()));
                        }
                    }
                }
                TimelineChange::Gone { item } => {
                    for member in members.values_mut() {
                        if member.as_ref().is_some_and(|place| place.item == item) {
                            *member = None;
                        }
                    }
                }
            }
        }
        after = through.min(to);
    }
    let first = staged.first();
    let last = staged.last();
    Ok(members
        .into_iter()
        .filter_map(|(key, member)| {
            let outside = member.is_none_or(|place| {
                first.as_ref().is_some_and(|first| place.order < first.order)
                    || last.as_ref().is_some_and(|last| place.order > last.order)
            });
            outside.then_some(key)
        })
        .collect())
}
