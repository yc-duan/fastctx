//! A conversation's timers (design §28.22, plan 3-4 group 9): each timer it
//! set, or its goal set, by the timer's id; the body is the timer as the
//! machine that fires it keeps it — when it is due, what it does, how its
//! firing went. Written by the machine the conversation lives on, which
//! fires its timers and alone orders the stream; one done or cancelled is
//! gone.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// The timers of a conversation.
pub struct ConversationTimer;

/// The id a timer's body says it has.
fn id_of(body: &[u8]) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|error| format!("a timer: {error}"))?;
    value.get("id").and_then(serde_json::Value::as_str).map(str::to_string).ok_or_else(|| "a timer says its id".into())
}

impl Kind for ConversationTimer {
    /// `conversation.timer` in the registry, whose order only grows.
    const KIND: u16 = 16;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "timers";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &[];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the machine's timer table.
    const IMPORT: Import = Import::From("timers");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        id_of(body)?;
        Ok(body.to_vec())
    }

    /// A timer is named by its id and refers to no object.
    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } => {
                if !objects.is_empty() {
                    return Err("a timer refers to no object".into());
                }
                if super::names::item_of(&id_of(body)?) != after.record.item {
                    return Err("a timer is named by its id".into());
                }
                Ok(())
            }
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("timers:{scope}")
    }
}
