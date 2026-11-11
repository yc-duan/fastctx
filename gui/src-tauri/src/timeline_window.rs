//! A bounded, disposable view of timeline pages. Positions stay opaque; the
//! numeric offsets exposed to the web page refer only to this held window.

use fastctx_types::{
    session::{Content, Happened, Part, SessionEvent, What},
    timeline_pages::{TimelineEvent, TimelinePlace, TimelineShown},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) const HELD_EVENTS: usize = 800;
pub(crate) const HELD_BYTES: usize = 4 << 20;

#[derive(Clone, Default, Serialize)]
pub(crate) struct Edges {
    pub older: bool,
    pub newer: bool,
    pub objects: Vec<Object>,
    pub keys: Vec<String>,
    pub live_keys: Vec<String>,
    pub pending: Vec<SessionEvent>,
}
#[derive(Clone, Serialize)]
pub(crate) struct Object {
    pub at: usize,
    pub bytes: u64,
    pub object: String,
}

#[derive(Clone)]
pub(crate) struct Stored {
    pub place: TimelinePlace,
    pub sequence: u64,
    pub event: Option<SessionEvent>,
    name: Option<String>,
    object: Option<(u64, String)>,
    bytes: usize,
}
impl Stored {
    pub fn key(&self) -> String {
        self.event
            .as_ref()
            .filter(|event| known_name(event).is_some())
            .map(key)
            .unwrap_or_else(|| format!("place:{}", self.place.item))
    }
    pub fn has_object(&self, hash: &str) -> bool {
        self.object.as_ref().is_some_and(|(_, object)| object == hash)
    }
}
impl From<TimelineEvent> for Stored {
    fn from(raw: TimelineEvent) -> Self {
        let event = decoded(&raw).or_else(|| {
            matches!(raw.shown, TimelineShown::Oversized { slim: None, .. }).then(|| SessionEvent {
                at: jiff::Timestamp::UNIX_EPOCH,
                happened: Happened::Finished {
                    turn: format!("timeline:{}", raw.place.item),
                    item: fastctx_types::session::Item {
                        id: raw.place.item.clone(),
                        draft: None,
                        started_at: None,
                        detail: None,
                        what: What::Other { host_kind: "fastctx:timeline-preview-unavailable".into() },
                    },
                },
            })
        });
        let name = event.as_ref().and_then(known_name);
        let bytes = serde_json::to_vec(&raw).map_or(0, |b| b.len());
        let object = match raw.shown {
            TimelineShown::Oversized { bytes, object, .. } => Some((bytes, object)),
            _ => None,
        };
        Self { place: raw.place, sequence: raw.sequence, event, name, object, bytes }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Window {
    pub entries: Vec<Stored>,
    pub interval_start: Option<TimelinePlace>,
    pub interval_end: Option<TimelinePlace>,
    pub live: Vec<SessionEvent>,
    live_base: BTreeMap<String, Option<u64>>,
    pending: BTreeMap<String, SessionEvent>,
    pending_bytes: usize,
    pub pending_over_budget: bool,
    pub revision: u64,
    pub older: bool,
    pub newer: bool,
    /// A known position immediately beyond the held right edge. Keeping this
    /// one cursor lets a revision reread exactly the bounded neighborhood.
    pub right: Option<TimelinePlace>,
    pub chunks: usize,
    pub previewed: bool,
    live_bytes: usize,
    live_sizes: BTreeMap<String, usize>,
    pub retired: Vec<String>,
}
impl Window {
    pub fn aliases(&self, event: &SessionEvent) -> Vec<String> {
        if !matches!(event.happened, Happened::Heard { .. }) {
            return Vec::new();
        }
        let current = key(event);
        self.entries
            .iter()
            .filter(|held| held.event.as_ref().is_some_and(|old| covers(event, old)))
            .map(Stored::key)
            .chain(self.live.iter().filter(|held| covers(event, held)).map(key))
            .filter(|key| key != &current)
            .collect()
    }
    pub fn object_visible(&self, hash: &str) -> bool {
        self.entries.iter().any(|entry| {
            entry.has_object(hash)
                && entry
                    .event
                    .as_ref()
                    .is_some_and(|event| self.newer || !self.live.iter().any(|live| covers(live, event)))
        })
    }
    pub fn first(&self) -> Option<TimelinePlace> {
        self.interval_start.clone().or_else(|| self.entries.first().map(|e| e.place.clone()))
    }
    pub fn last(&self) -> Option<TimelinePlace> {
        self.interval_end.clone().or_else(|| self.entries.last().map(|e| e.place.clone()))
    }

    pub fn replace(&mut self, events: Vec<TimelineEvent>, revision: u64, older: bool, newer: bool) {
        if let Some(first) = events.first() {
            self.interval_start = Some(first.place.clone());
        }
        if let Some(last) = events.last() {
            self.interval_end = Some(last.place.clone());
        }
        self.entries.clear();
        self.revision = revision;
        self.older = older;
        self.newer = newer;
        self.merge(events, false);
    }

    pub fn merge(&mut self, events: Vec<TimelineEvent>, backwards: bool) {
        for event in events.into_iter().map(Stored::from) {
            if let Some(at) = self.entries.iter().position(|held| {
                held.place.item == event.place.item
                    || event.name.as_ref().is_some_and(|name| held.name.as_ref() == Some(name))
                    || event.event.as_ref().is_some_and(|incoming| {
                        matches!(incoming.happened, Happened::Heard { .. })
                            && held.event.as_ref().is_some_and(|old| covers(incoming, old))
                    })
            }) {
                if self.entries[at].sequence <= event.sequence {
                    self.entries[at] = event;
                }
            } else {
                self.entries.push(event);
            }
        }
        self.entries.sort_by(|a, b| (&a.place.order, &a.place.item).cmp(&(&b.place.order, &b.place.item)));
        self.reconcile();
        if let Some(first) = self.entries.first()
            && self
                .interval_start
                .as_ref()
                .is_none_or(|held| (&first.place.order, &first.place.item) < (&held.order, &held.item))
        {
            self.interval_start = Some(first.place.clone());
        }
        if let Some(last) = self.entries.last()
            && self
                .interval_end
                .as_ref()
                .is_none_or(|held| (&last.place.order, &last.place.item) > (&held.order, &held.item))
        {
            self.interval_end = Some(last.place.clone());
        }
        self.bound(backwards);
    }

    pub fn inherit_live(&mut self, current: &Self) {
        self.live = current.live.clone();
        self.live_base = current.live_base.clone();
        self.live_sizes = current.live_sizes.clone();
        self.live_bytes = current.live_bytes;
        self.pending = current.pending.clone();
        self.pending_bytes = current.pending_bytes;
        self.pending_over_budget = false;
        self.reconcile();
    }
    fn reconcile(&mut self) {
        let held = self.live.len();
        self.live.retain(|live| {
            let before = self.live_base.get(&name(live)).copied().flatten();
            let replaced = self.entries.iter().any(|held| {
                before.is_none_or(|before| held.sequence > before)
                    && held.event.as_ref().is_some_and(|event| covers(event, live))
            });
            if replaced {
                self.retired.push(key(live));
            }
            !replaced
        });
        if self.live.len() != held {
            self.recount_live();
        }
        self.prune_bases();
    }

    pub fn inherit_reread(&mut self, current: &Self, reset_sequence: bool) {
        self.inherit_live(current);
        let removed: Vec<_> = current
            .entries
            .iter()
            .filter_map(|e| e.event.as_ref())
            .filter(|old| !self.entries.iter().filter_map(|e| e.event.as_ref()).any(|event| covers(event, old)))
            .cloned()
            .collect();
        self.retain_live(|live| !removed.iter().any(|old| covers(old, live)));
        if reset_sequence {
            for baseline in self.live_base.values_mut() {
                *baseline = None;
            }
        }
        self.reconcile();
    }
    pub fn retain_live(&mut self, keep: impl Fn(&SessionEvent) -> bool) {
        self.live.retain(|event| {
            if keep(event) {
                return true;
            }
            self.retired.push(key(event));
            false
        });
        self.recount_live();
        self.prune_bases();
    }
    fn recount_live(&mut self) {
        self.live_sizes = self.live.iter().map(|e| (key(e), serde_json::to_vec(e).map_or(0, |v| v.len()))).collect();
        self.live_bytes = self.live_sizes.values().sum();
    }

    fn prune_bases(&mut self) {
        let held: std::collections::BTreeSet<_> = self.live.iter().map(name).collect();
        self.live_base.retain(|key, _| held.contains(key));
    }

    fn bound(&mut self, backwards: bool) {
        let mut bytes: usize = self.entries.iter().map(|e| e.bytes).sum();
        while self.entries.len() > HELD_EVENTS || bytes > HELD_BYTES {
            if self.entries.is_empty() {
                break;
            }
            let removed = if backwards { self.entries.pop().expect("held event") } else { self.entries.remove(0) };
            bytes = bytes.saturating_sub(removed.bytes);
            if backwards {
                self.newer = true;
                self.right = Some(removed.place);
                self.interval_end = self.entries.last().map(|e| e.place.clone());
            } else {
                self.older = true;
                self.interval_start = self.entries.first().map(|e| e.place.clone());
            }
        }
    }

    /// Coalesce streaming chunks into their running snapshot, rather than
    /// keeping a journal whose size grows with the number of chunks.
    pub fn pending(&mut self, events: Vec<SessionEvent>) {
        self.pending.clear();
        self.pending_bytes = 0;
        self.pending_over_budget = false;
        for event in events {
            self.observe(&event);
        }
    }
    pub fn observe(&mut self, event: &SessionEvent) -> bool {
        self.pending_over_budget = false;

        use fastctx_types::session::SessionChange;
        match &event.happened {
            Happened::Changed { change: SessionChange::Request { id, .. }, .. } => {
                let key = id.to_string();
                let was = self.pending.get(&key).map_or(0, |e| serde_json::to_vec(e).map_or(0, |v| v.len()));
                let bytes = serde_json::to_vec(event).map_or(0, |v| v.len());
                if (self.pending.len() >= HELD_EVENTS && !self.pending.contains_key(&key))
                    || self.pending_bytes - was + bytes > HELD_BYTES
                {
                    self.pending_over_budget = true;
                    return false;
                }
                self.pending_bytes = self.pending_bytes - was + bytes;
                self.pending.insert(key, event.clone());
                return true;
            }
            Happened::Changed { change: SessionChange::Resolved { id, .. }, .. } => {
                if let Some(event) = self.pending.remove(&id.to_string()) {
                    self.pending_bytes =
                        self.pending_bytes.saturating_sub(serde_json::to_vec(&event).map_or(0, |v| v.len()));
                }
                return true;
            }
            _ => {}
        }
        if self.entries.iter().filter_map(|held| held.event.as_ref()).chain(self.live.iter()).any(|e| {
            e == event
                || (matches!(
                    (&e.happened, &event.happened),
                    (
                        Happened::Finished { .. },
                        Happened::Began { .. } | Happened::Updated { .. } | Happened::Grew { .. }
                    )
                ) && covers(e, event))
        }) {
            return false;
        }
        // Accept a running page snapshot without losing its prefix on growth.
        if let Happened::Grew { turn, item, .. } = &event.happened
            && !self.live.iter().any(|e| matches!(&e.happened,
                Happened::Began { turn: t, item: i } | Happened::Updated { turn: t, item: i } if t == turn && i.id == *item))
            && let Some(snapshot) = self.entries.iter().filter_map(|e| e.event.as_ref()).rev().find(|e| matches!(&e.happened,
                Happened::Began { turn: t, item: i } | Happened::Updated { turn: t, item: i } if t == turn && i.id == *item)).cloned()
        {
            self.live.push(snapshot);
            self.recount_live();
        }
        if let Happened::Grew { turn, item, part, text, .. } = &event.happened
            && let Some(snapshot) = self.live.iter_mut().rev().find(|e| matches!(&e.happened,
                Happened::Began { turn: t, item: i } | Happened::Updated { turn: t, item: i } if t == turn && i.id == *item)) {
                let (Happened::Began { item, .. } | Happened::Updated { item, .. }) = &mut snapshot.happened else { unreachable!() };
                match (&mut item.what, part) {
                    (What::Message { text: whole } | What::Thinking { text: whole, .. } | What::Plan { text: whole }, Part::Text) => whole.push_str(text),
                    (What::Tool(tool), Part::Output) => match tool.output.last_mut() {
                        Some(Content::Text { text: whole }) => whole.push_str(text),
                        _ => tool.output.push(Content::Text { text: text.clone() }),
                    },
                    _ => {},
                }
                self.previewed |= crate::timeline_preview::limit(snapshot);
                let key = key(snapshot);
                let bytes = serde_json::to_vec(snapshot).map_or(0, |v| v.len());
                self.live_bytes = self.live_bytes + bytes - self.live_sizes.insert(key, bytes).unwrap_or(0);
                self.bound_live();
                self.chunks += 1;
                return true;
        }
        if let Happened::Grew { turn, item, part, text, .. } = &event.happened {
            if *part != Part::Text {
                return false;
            }
            return self.observe(&SessionEvent {
                at: event.at,
                happened: Happened::Began {
                    turn: turn.clone(),
                    item: fastctx_types::session::Item {
                        id: item.clone(),
                        draft: None,
                        started_at: None,
                        detail: None,
                        what: What::Message { text: text.clone() },
                    },
                },
            });
        }
        let replaced = self.live.iter().position(|held| covers(event, held));
        self.live.retain(|held| {
            if !covers(event, held) {
                return true;
            }
            self.live_bytes = self.live_bytes.saturating_sub(self.live_sizes.remove(&key(held)).unwrap_or(0));
            self.retired.push(key(held));
            false
        });
        let before = self
            .entries
            .iter()
            .filter(|held| held.event.as_ref().is_some_and(|e| covers(e, event)))
            .map(|held| held.sequence)
            .max();
        self.live_base.insert(name(event), before);
        self.live.insert(replaced.unwrap_or(self.live.len()).min(self.live.len()), event.clone());
        let bytes = serde_json::to_vec(event).map_or(0, |v| v.len());
        self.live_bytes += bytes;
        self.live_sizes.insert(key(event), bytes);
        self.bound_live();
        self.prune_bases();
        self.chunks += 1;
        true
    }

    pub fn trim_visible(&mut self) -> Vec<String> {
        let mut bytes: usize = self.entries.iter().map(|e| e.bytes).sum::<usize>() + self.live_bytes;
        let mut removed = Vec::new();
        let overlaps = self
            .entries
            .iter()
            .filter(|held| held.event.as_ref().is_some_and(|event| self.live.iter().any(|live| covers(live, event))))
            .count();
        let mut count = self.entries.len() + self.live.len() - overlaps;
        while !self.entries.is_empty() && (count > HELD_EVENTS || bytes > HELD_BYTES) {
            let entry = self.entries.remove(0);
            bytes = bytes.saturating_sub(entry.bytes);
            self.interval_start = self.entries.first().map(|e| e.place.clone()).or_else(|| self.interval_end.clone());
            let entry_key = entry.key();
            if let Some(event) = entry.event {
                if !self.live.iter().any(|live| covers(live, &event)) {
                    removed.push(entry_key);
                    count -= 1;
                }
            } else {
                count -= 1;
            }
            self.older = true;
        }
        removed
    }
    pub fn pending_events(&self) -> Vec<SessionEvent> {
        self.pending.values().cloned().collect()
    }
    pub fn ordered_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for entry in &self.entries {
            if let Some(event) = &entry.event {
                let overlay = (!self.newer).then(|| self.live.iter().find(|live| covers(live, event))).flatten();
                keys.push(overlay.map_or_else(|| entry.key(), key));
            }
        }
        if !self.newer {
            keys.extend(
                self.live
                    .iter()
                    .filter(|event| {
                        !self.entries.iter().filter_map(|e| e.event.as_ref()).any(|held| covers(event, held))
                    })
                    .map(key),
            );
        }
        keys
    }
    pub fn live_keys(&self) -> Vec<String> {
        self.live.iter().map(key).collect()
    }
    pub fn running(&self, event: &SessionEvent) -> Option<SessionEvent> {
        let Happened::Grew { turn, item, .. } = &event.happened else { return None };
        self.live.iter().find(|e| matches!(&e.happened, Happened::Began { turn: t, item: i } | Happened::Updated { turn: t, item: i } if t == turn && i.id == *item)).cloned()
    }
    fn bound_live(&mut self) {
        while self.live.len() > HELD_EVENTS / 2 || self.live_bytes > HELD_BYTES / 2 {
            if self.live.is_empty() {
                break;
            }
            let removed = self.live.remove(0);
            self.retired.push(key(&removed));
            self.live_bytes = self.live_bytes.saturating_sub(self.live_sizes.remove(&key(&removed)).unwrap_or(0));
        }
    }

    pub fn shown(&mut self) -> Result<(Vec<SessionEvent>, Edges), String> {
        self.bound_live();
        let removed = self.trim_visible();
        self.retired.extend(removed);
        let mut events = Vec::new();
        let mut edges = Edges {
            older: self.older,
            newer: self.newer,
            objects: Vec::new(),
            keys: Vec::new(),
            live_keys: self.live_keys(),
            pending: self.pending_events(),
        };
        for entry in &self.entries {
            if let Some(event) = &entry.event {
                let overlay = (!self.newer).then(|| self.live.iter().find(|live| covers(live, event))).flatten();
                if overlay.is_none()
                    && let Some((bytes, object)) = &entry.object
                {
                    edges.objects.push(Object { at: events.len(), bytes: *bytes, object: object.clone() });
                }
                let shown_key = overlay.map_or_else(|| entry.key(), key);
                let event = overlay.unwrap_or(event);
                events.push(event.clone());
                edges.keys.push(shown_key);
            }
        }

        if !self.newer {
            for event in &self.live {
                if self.entries.iter().filter_map(|e| e.event.as_ref()).any(|held| covers(event, held)) {
                    continue;
                }
                events.push(event.clone());
                edges.keys.push(key(event));
            }
        }

        self.recount_live();
        edges.live_keys = self.live_keys();
        self.prune_bases();
        self.chunks = 0;
        self.previewed = false;
        Ok((events, edges))
    }
}

pub(crate) fn key(event: &SessionEvent) -> String {
    format!("event:{:x}", Sha256::digest(name(event).as_bytes()))
}
pub(crate) fn pending_event(event: &SessionEvent) -> bool {
    matches!(
        event.happened,
        Happened::Changed {
            change: fastctx_types::session::SessionChange::Request { .. }
                | fastctx_types::session::SessionChange::Resolved { .. },
            ..
        }
    )
}

pub(crate) fn decoded(event: &TimelineEvent) -> Option<SessionEvent> {
    let value = match &event.shown {
        TimelineShown::Event { event } => event,
        TimelineShown::Oversized { slim: Some(event), .. } => event,
        _ => return None,
    };
    serde_json::from_value(value.clone()).ok()
}
fn name(event: &SessionEvent) -> String {
    known_name(event).unwrap_or_else(|| serde_json::to_string(event).unwrap_or_default())
}
fn known_name(event: &SessionEvent) -> Option<String> {
    match &event.happened {
        Happened::Finished { turn, item } if !item.id.is_empty() => Some(format!("finished:{turn}:{}", item.id)),
        Happened::Began { turn, item } | Happened::Updated { turn, item } if !item.id.is_empty() => {
            Some(format!("running:{turn}:{}", item.id))
        }
        Happened::Grew { turn, item, .. } if !item.is_empty() => Some(format!("running:{turn}:{item}")),
        Happened::TurnStarted { turn } => Some(format!("started:{turn}")),
        Happened::TurnEnded { turn, .. } => Some(format!("ended:{turn}")),
        Happened::Heard { delivery: Some(id), .. } => Some(format!("delivery:{id}")),
        Happened::Heard { message: Some(message), .. } => Some(format!("message:{}", message.id)),
        _ => None,
    }
}
pub(crate) fn covers(new: &SessionEvent, old: &SessionEvent) -> bool {
    match (&new.happened, &old.happened) {
        (
            Happened::Finished { turn, item },
            Happened::Began { turn: t, item: i } | Happened::Updated { turn: t, item: i },
        ) => turn == t && (item.id == i.id || item.draft.as_ref() == Some(&i.id)),
        (Happened::Finished { turn, item }, Happened::Grew { turn: t, item: i, .. }) => {
            turn == t && (item.id == *i || item.draft.as_ref() == Some(i))
        }
        (
            Happened::Finished { item, .. },
            Happened::Changed { change: fastctx_types::session::SessionChange::RunFactsUpdated { update, .. }, .. },
        ) => item.id == update.call_id,
        (Happened::Heard { delivery: a, message: b, .. }, Happened::Heard { delivery: x, message: y, .. }) => {
            a.as_ref().is_some_and(|a| Some(a) == x.as_ref())
                || b.as_ref().is_some_and(|b| y.as_ref().is_some_and(|y| b.id == y.id))
        }
        _ => match (known_name(new), known_name(old)) {
            (Some(new), Some(old)) => new == old,
            (None, None) => new == old,
            _ => false,
        },
    }
}
