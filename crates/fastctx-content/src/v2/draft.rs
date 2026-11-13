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

fn object(body: &[u8]) -> Result<(), String> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(map)) if map.get("text").is_some_and(serde_json::Value::is_string) => Ok(()),
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
    const ROLES: &'static [&'static str] = &[];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the conversation's composer.
    const IMPORT: Import = Import::From("composer");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        object(body).map(|()| body.to_vec())
    }

    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } if objects.is_empty() => object(body),
            State::Is { .. } => Err("a draft refers to no object".into()),
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("draft:{scope}")
    }
}
