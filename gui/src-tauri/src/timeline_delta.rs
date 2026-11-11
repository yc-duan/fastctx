//! Materialization updates only changed events. Navigation alone replaces a
//! window and advances its generation; a revision is not a navigation.

use crate::timeline_window::{Edges, Window};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize)]
pub(crate) struct Changed {
    pub key: String,
    pub event: Box<serde_json::value::RawValue>,
    pub place: Option<fastctx_types::timeline_pages::TimelinePlace>,
}
#[derive(Clone, Serialize)]
pub(crate) struct Placement {
    pub key: String,
    pub after: Option<String>,
}
#[derive(Clone, Serialize)]
pub(crate) struct Delta {
    pub generation: u64,
    pub events: Vec<Changed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<Vec<fastctx_types::session::SessionEvent>>,
    pub removed: Vec<String>,
    pub placed: Vec<Placement>,
    /// Bounds, deferred objects and pending requests. Keys are conveyed only
    /// by the membership changes, rather than resending every held identity.
    pub pages: Edges,
}

pub(crate) fn between(before: &Window, after: &mut Window, generation: u64) -> Result<Delta, String> {
    let old_live: BTreeSet<_> = before.live_keys().into_iter().collect();
    let versions = |window: &Window| -> BTreeMap<_, _> {
        window
            .entries
            .iter()
            .filter_map(|e| e.event.as_ref().map(|_| (e.key(), (e.place.item.clone(), e.sequence))))
            .collect()
    };
    let old = versions(before);
    let new = versions(after);
    let old_order = before.ordered_keys();
    let (events, mut pages) = after.shown()?;
    let live: BTreeSet<_> = pages.live_keys.iter().cloned().collect();
    let events = events
        .iter()
        .zip(&pages.keys)
        .enumerate()
        .filter_map(|(at, (event, key))| {
            let changed = if live.contains(key) {
                !old_live.contains(key)
                    || before.live.iter().find(|event| crate::timeline_window::key(event) == *key)
                        != after.live.iter().find(|event| crate::timeline_window::key(event) == *key)
            } else {
                old_live.contains(key) || old.get(key) != new.get(key)
            };
            changed.then(|| {
                serde_json::value::to_raw_value(event)
                    .map(|event| Changed { key: key.clone(), event, place: pages.places.get(at).cloned().flatten() })
                    .map_err(|e| e.to_string())
            })
        })
        .collect::<Result<_, _>>()?;
    let retained: BTreeSet<_> = pages.keys.iter().collect();
    let removed = old_order.iter().filter(|key| !retained.contains(key)).cloned().collect();
    let mut order: Vec<_> = old_order.iter().filter(|key| retained.contains(key)).cloned().collect();
    let mut placed = Vec::new();
    for (at, key) in pages.keys.iter().enumerate() {
        if order.get(at) == Some(key) {
            continue;
        }
        if let Some(old) = order.iter().position(|held| held == key) {
            order.remove(old);
        }
        order.insert(at, key.clone());
        placed.push(Placement { key: key.clone(), after: at.checked_sub(1).map(|at| pages.keys[at].clone()) });
    }
    let pending = (before.pending_events() != pages.pending).then(|| pages.pending.clone());
    pages.pending.clear();
    pages.keys.clear();
    pages.places.clear();
    pages.live_keys.clear();
    Ok(Delta { generation, events, pending, removed, placed, pages })
}
