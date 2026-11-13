//! The kinds of a conversation's plan and notes in the asset library (design
//! §28.17): the hub's account records are where they are written and merged;
//! the hub writes each record as it accepts it into a stream it orders in
//! the conversation's scope, the plan's one item in `plan`, each note by its
//! name in `history`. A body is the account record itself — its version,
//! who wrote it and when, and what it says.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// A conversation's plan.
pub struct ConversationPlan;

/// A conversation's notes, one item each.
pub struct ConversationHistory;

fn record(body: &[u8]) -> Result<Vec<u8>, String> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) if value.get("metadata").is_some_and(serde_json::Value::is_object) => Ok(body.to_vec()),
        Ok(_) => Err("a document's record carries its metadata".into()),
        Err(error) => Err(format!("a document's record is not JSON: {error}")),
    }
}

fn judged(after: &KindRecord) -> Result<(), String> {
    match &after.record.state {
        State::Gone => Ok(()),
        State::Is { body, objects } if objects.is_empty() => record(body).map(|_| ()),
        State::Is { .. } => Err("a document's record refers to no object".into()),
    }
}

impl Kind for ConversationPlan {
    /// `conversation.plan` in the registry (`fastctx_sync::registry`), whose
    /// order only grows.
    const KIND: u16 = 12;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "plan";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &[];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the hub's account records.
    const IMPORT: Import = Import::From("account-records");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        record(body)
    }

    /// The hub alone orders the stream (`HUB_ORDERED`): what is judged is
    /// that each record is an account record.
    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        judged(after)
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("plan:{scope}")
    }
}

impl Kind for ConversationHistory {
    /// `conversation.history` in the registry.
    const KIND: u16 = 13;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "history";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &[];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    const IMPORT: Import = Import::From("account-records");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        record(body)
    }

    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        judged(after)
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("history:{scope}")
    }
}
