//! A conversation's input-box draft (design §28.24, plan 3-4 group 4): what
//! is typed and attached for the next words, as it stands, one item. The
//! draft is merged where the conversation lives — every device edits it
//! live there — so the item is that machine's snapshot of it, written as it
//! changes: on the devices that hold the conversation, and on the hub for
//! good, for one that comes back to it elsewhere.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// A conversation's draft.
pub struct ConversationDraft;

/// Whether a draft's body is the draft, or says the draft is its one
/// object (AS16).
fn object(body: &[u8]) -> Result<bool, String> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(map)) if map.get("in_object").and_then(serde_json::Value::as_bool) == Some(true) => {
            Ok(true)
        }
        Ok(serde_json::Value::Object(map)) if map.get("text").is_some_and(serde_json::Value::is_string) => Ok(false),
        Ok(_) => Err("a draft is a JSON object with its text".into()),
        Err(error) => Err(format!("a draft is not JSON: {error}")),
    }
}

impl Kind for ConversationDraft {
    /// `conversation.draft` in the registry, whose order only grows.
    const KIND: u16 = 11;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "draft";
    const HOLDING: Holding = Holding::Readers;
    /// A draft larger than a body holds is its one object.
    const ROLES: &'static [&'static str] = &["draft"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the conversation's composer.
    const IMPORT: Import = Import::From("composer");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        object(body).map(|_| body.to_vec())
    }

    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } => match (object(body)?, objects.len()) {
                (false, 0) | (true, 1) => Ok(()),
                _ => Err("a draft is its body, or the one object its body says it is".into()),
            },
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("draft:{scope}")
    }
}
