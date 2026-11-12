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

/// The identity of each event of `history`, in its order.
pub(super) fn identities(history: &[SessionEvent]) -> Vec<Vec<u8>> {
    let mut seen: HashMap<[u8; 32], u32> = HashMap::new();
    history
        .iter()
        .map(|event| {
            let mut hasher = Sha256::new();
            hasher.update(b"fastctx-timeline/2");
            match super::identity::name(event).or_else(|| named(&event.happened)) {
                Some(name) => {
                    hasher.update(b"named");
                    hasher.update(name.as_bytes());
                }
                None => {
                    hasher.update(b"moment");
                    hasher.update(event.at.as_nanosecond().to_le_bytes());
                    hasher.update(kind(&event.happened).as_bytes());
                }
            }
            let named: [u8; 32] = hasher.finalize().into();
            let before = seen.entry(named).or_insert(0);
            let identity = if *before == 0 {
                named
            } else {
                let mut again = Sha256::new();
                again.update(named);
                again.update(before.to_le_bytes());
                again.finalize().into()
            };
            *before += 1;
            identity.to_vec()
        })
        .collect()
}
