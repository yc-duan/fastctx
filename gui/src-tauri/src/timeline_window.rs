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
    sequence: u64,
    event: Option<SessionEvent>,
    name: Option<String>,
    object: Option<(u64, String)>,
    bytes: usize,
}
impl From<TimelineEvent> for Stored {
    fn from(raw: TimelineEvent) -> Self {
        let event = decoded(&raw).or_else(|| {
            matches!(raw.shown, TimelineShown::Oversized { .. }).then(|| SessionEvent {
                at: jiff::Timestamp::UNIX_EPOCH,
                happened: Happened::Finished {
                    turn: String::new(),
                    item: fastctx_types::session::Item {
                        id: raw.place.item.clone(),
                        draft: None,
                        started_at: None,
                        detail: None,
                        what: What::Other { host_kind: "Content preview unavailable".into() },
                    },
                },
            })
        });
        let name = event.as_ref().map(name);
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
    pub live: Vec<SessionEvent>,
    live_base: BTreeMap<String, u64>,
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
}
impl Window {
    pub fn first(&self) -> Option<TimelinePlace> {
        self.entries.first().map(|e| e.place.clone())
    }
    pub fn last(&self) -> Option<TimelinePlace> {
        self.entries.last().map(|e| e.place.clone())
    }

    pub fn replace(&mut self, events: Vec<TimelineEvent>, revision: u64, older: bool, newer: bool) {
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
        self.bound(backwards);
    }

    fn reconcile(&mut self) {
        self.live.retain(|live| {
            let before = self.live_base.get(&name(live)).copied().unwrap_or(0);
            !self
                .entries
                .iter()
                .any(|held| held.sequence > before && held.event.as_ref().is_some_and(|event| covers(event, live)))
        });
        self.prune_bases();
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
            } else {
                self.older = true;
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
        if self.entries.iter().any(|held| {
            held.event.as_ref().is_some_and(|e| {
                e == event
                    || (e.at >= event.at
                        && matches!(
                            (&e.happened, &event.happened),
                            (
                                Happened::Finished { .. },
                                Happened::Began { .. } | Happened::Updated { .. } | Happened::Grew { .. }
                            )
                        )
                        && covers(e, event))
            })
        }) {
            return false;
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
                self.chunks += 1;
                return true;
        }
        self.live.retain(|held| !covers(event, held));
        let before = self
            .entries
            .iter()
            .filter(|held| held.event.as_ref().is_some_and(|e| covers(e, event)))
            .map(|held| held.sequence)
            .max()
            .unwrap_or(0);
        self.live_base.insert(name(event), before);
        self.live.push(event.clone());
        self.chunks += 1;
        true
    }

    pub fn shown(&mut self) -> Result<(Vec<SessionEvent>, Edges), String> {
        let mut events = Vec::new();
        let mut edges = Edges { older: self.older, newer: self.newer, objects: Vec::new(), keys: Vec::new() };
        for entry in &self.entries {
            if let Some(event) = &entry.event {
                if let Some((bytes, object)) = &entry.object {
                    edges.objects.push(Object { at: events.len(), bytes: *bytes, object: object.clone() });
                }
                events.push(event.clone());
                edges.keys.push(entry.place.item.clone());
            }
        }
        let mut persisted = events.len();
        if !self.newer {
            for event in self.live.iter().chain(self.pending.values()) {
                events.push(event.clone());
                edges.keys.push(format!("live:{:x}", Sha256::digest(serde_json::to_vec(event).unwrap_or_default())));
            }
        }
        let mut bytes: usize = events.iter().map(|e| serde_json::to_vec(e).map_or(0, |b| b.len())).sum();
        while events.len() > HELD_EVENTS || bytes > HELD_BYTES {
            if events.len() <= 1 {
                return Err("the current live event exceeds the conversation window's 4 MiB limit".into());
            }
            let removed = events.remove(0);
            edges.keys.remove(0);
            if persisted > 0 {
                while !self.entries.is_empty() {
                    if self.entries.remove(0).event.is_some() {
                        break;
                    }
                }
                persisted -= 1;
                self.older = true;
            } else if !self.live.is_empty() {
                self.live.remove(0);
            }
            bytes = bytes.saturating_sub(serde_json::to_vec(&removed).map_or(0, |b| b.len()));
            edges.objects.retain_mut(|o| {
                if o.at == 0 {
                    false
                } else {
                    o.at -= 1;
                    true
                }
            });
            edges.older = true;
        }
        // A completed live event remains only until its materialized counterpart
        // arrives, and the overlay itself has the same hard bounds as a page.
        let mut live_bytes: usize = self.live.iter().map(|e| serde_json::to_vec(e).map_or(0, |b| b.len())).sum();
        while self.live.len() > HELD_EVENTS / 2 || live_bytes > HELD_BYTES / 2 {
            if self.live.len() <= 1 {
                return Err("the current live event exceeds the conversation window's 4 MiB limit".into());
            }
            live_bytes -= serde_json::to_vec(&self.live.remove(0)).map_or(0, |b| b.len());
        }
        self.prune_bases();
        self.chunks = 0;
        self.previewed = false;
        Ok((events, edges))
    }
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
    match &event.happened {
        Happened::Finished { turn, item } => format!("finished:{turn}:{}", item.id),
        Happened::Began { turn, item } | Happened::Updated { turn, item } => format!("running:{turn}:{}", item.id),
        Happened::TurnStarted { turn } => format!("started:{turn}"),
        Happened::TurnEnded { turn, .. } => format!("ended:{turn}"),
        Happened::Heard { delivery: Some(id), .. } => format!("delivery:{id}"),
        Happened::Heard { message: Some(message), .. } => format!("message:{}", message.id),
        _ => serde_json::to_string(event).unwrap_or_default(),
    }
}
fn covers(new: &SessionEvent, old: &SessionEvent) -> bool {
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
        _ => name(new) == name(old),
    }
}
