//! What each event of a conversation's timeline in the asset library is
//! known by (sync-v2-impl-design §12). An event that says what it is — an
//! item, words handed over, a turn's start or end — is known by that; any
//! other by when it happened and what kind of event it is, events alike at
//! one moment told apart by their order. No name depends on the event's
//! content, so a changed event is a new revision of the same item, nor on
//! what came long before, so a history read past a seam (`super::tail`)
//! names its events as a whole reading does.

use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// The kind of event `happened` is, as its encoding names it: a change, by
/// the kind of change.
fn kind(happened: &Happened) -> String {
    let value = serde_json::to_value(happened).unwrap_or_default();
    let event = value.get("event").and_then(serde_json::Value::as_str).unwrap_or("event");
    match value.pointer("/change/kind").and_then(serde_json::Value::as_str) {
        Some(change) => format!("{event}/{change}"),
        None => event.to_string(),
    }
}

/// What names an event that says what it is beyond what the content
/// database's timeline names (`identity::name`): a response's usage, by the
/// response, so several at one moment do not take each other's places.
fn named(happened: &Happened) -> Option<String> {
    match happened {
        Happened::Changed { change: SessionChange::MessageUsage { items, response, .. }, .. } => Some(match response {
            Some(response) => format!("usage/{response}"),
            None => format!("usage/{}", items.join("\u{1f}")),
        }),
        _ => None,
    }
}

/// What names `event` whatever its moment, where something does: the item,
/// words or turn it is, or the response a usage is of (`identities`).
pub(crate) fn key(event: &SessionEvent) -> Option<String> {
    super::identity::name(event).or_else(|| named(&event.happened))
}

/// What `event` is known by where a record stopped short (§28.49) keeps the
/// timeline's own: its name (`key`), else its moment and kind, as
/// `identities` knows it but for the order among events alike.
pub(crate) fn frozen(event: &SessionEvent) -> String {
    key(event).unwrap_or_else(|| format!("moment/{}/{}", event.at.as_nanosecond(), kind(&event.happened)))
}

/// The identity of each event of `history`, in its order.
pub(super) fn identities(history: &[SessionEvent]) -> Vec<Vec<u8>> {
    let mut identities = Identities::default();
    history.iter().map(|event| identities.next(event).to_vec()).collect()
}

/// The identities of a history's events handed one at a time, in its order
/// (`identities`): of what came before, only how many were alike is kept.
#[derive(Default)]
pub(super) struct Identities {
    seen: HashMap<[u8; 32], u32>,
}

impl Identities {
    /// The identity of `event`, the next of the history.
    pub(super) fn next(&mut self, event: &SessionEvent) -> [u8; 32] {
        let base = base(event);
        let before = self.seen.entry(base).or_insert(0);
        let identity = again(base, *before);
        *before += 1;
        identity
    }
}

/// What `event` is known by before those alike that came before it are
/// counted (`again`): what names it (`key`), else its moment and kind.
pub(super) fn base(event: &SessionEvent) -> [u8; 32] {
    match super::identity::name(event).or_else(|| named(&event.happened)) {
        Some(name) => named_base(&name),
        None => {
            let mut hasher = Sha256::new();
            hasher.update(b"fastctx-timeline/2");
            hasher.update(b"moment");
            hasher.update(event.at.as_nanosecond().to_le_bytes());
            hasher.update(kind(&event.happened).as_bytes());
            hasher.finalize().into()
        }
    }
}

/// What an event `name` names is known by (`base`), before those alike are
/// counted: the item a host's call finished is, `item/{call}`, say.
pub(super) fn named_base(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"fastctx-timeline/2");
    hasher.update(b"named");
    hasher.update(name.as_bytes());
    hasher.finalize().into()
}

/// The identity of an event known by `base`, `before` events of the history
/// known by it before it: the first, by its base; each after, by its base
/// and how many came before.
pub(super) fn again(base: [u8; 32], before: u32) -> [u8; 32] {
    if before == 0 {
        return base;
    }
    let mut again = Sha256::new();
    again.update(base);
    again.update(before.to_le_bytes());
    again.finalize().into()
}
