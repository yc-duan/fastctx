//! The kind of a conversation's timeline (design §2.4, §10, §11): each
//! event of its history one item, named by what the event says it is so a
//! changed event is a new revision of the same item. Its stream is ordered by
//! the machine the conversation lives on, which alone writes it; a body is
//! the event as JSON after `EVENT`, or, for an event too large for a record,
//! what is shown in its place after `OVERSIZED`, the whole event its one
//! object.

use super::kind::{Actor, Deletion, Holding, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// A body that is the event itself, as JSON after this byte.
pub const EVENT: u8 = b'E';
/// A body that stands for an event kept whole as its one object: after this
/// byte, its length and what is shown in its place, as JSON.
pub const OVERSIZED: u8 = b'O';

/// A conversation's timeline.
pub struct Timeline;

/// Whether `body` is a timeline body: its first byte says which, and what
/// follows is one JSON object.
fn well_formed(body: &[u8]) -> Result<u8, String> {
    let (&tag, json) = body.split_first().ok_or("a timeline body is not empty")?;
    if tag != EVENT && tag != OVERSIZED {
        return Err(format!("a timeline body begins with E or O, not {tag:#04x}"));
    }
    match serde_json::from_slice::<serde_json::Value>(json) {
        Ok(serde_json::Value::Object(_)) => Ok(tag),
        Ok(_) => Err("a timeline body holds one JSON object".into()),
        Err(error) => Err(format!("a timeline body is not JSON: {error}")),
    }
}

impl Kind for Timeline {
    /// `conversation.timeline` in the registry (`fastctx_sync::registry`),
    /// whose order only grows.
    const KIND: u16 = 7;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "timeline";
    const HOLDING: Holding = Holding::Readers;
    /// An oversized event's whole JSON.
    const ROLES: &'static [&'static str] = &["event"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        well_formed(body)?;
        Ok(body.to_vec())
    }

    /// Only the device that orders the stream writes it (the library takes
    /// frames of an epoch from that device alone), and it may change any of
    /// an item: what is judged is that the record is a timeline record.
    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } => match (well_formed(body)?, objects.len()) {
                (EVENT, 0) | (OVERSIZED, 1) => Ok(()),
                (EVENT, _) => Err("an event kept whole in its record refers to no object".into()),
                _ => Err("an oversized event refers to its one object".into()),
            },
        }
    }
}
