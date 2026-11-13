//! The kind of the user's words to a conversation (design §3, §28.14): one
//! item for each thing the user said, in the conversation's scope beside its
//! timeline, named by its notice id. The machine the conversation lives on
//! writes it as the words arrive, with their operation in the same
//! transaction, and then where their delivery stands; a body is one JSON
//! object whose `state` says so. Words let go before they went in are gone.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// Where the user's words can stand.
pub const WORDS_STATES: [&str; 6] = ["kept", "delivering", "delivered", "uncertain", "refused", "gone"];

/// The longest name an item of words has.
const NAME_BYTES: usize = 256;

/// The user's words to a conversation.
pub struct ConversationWords;

impl Kind for ConversationWords {
    /// `conversation.words` in the registry (`fastctx_sync::registry`),
    /// whose order only grows.
    const KIND: u16 = 52;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "words";
    const HOLDING: Holding = Holding::Readers;
    /// Words larger than a body holds keep their delivery as the one object.
    const ROLES: &'static [&'static str] = &["delivery"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// What the inbox kept before is taken up from it.
    const IMPORT: Import = Import::From("inbox");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        let value: serde_json::Value =
            serde_json::from_slice(body).map_err(|error| format!("words are not JSON: {error}"))?;
        match value.get("state").and_then(serde_json::Value::as_str) {
            Some(state) if WORDS_STATES.contains(&state) => Ok(body.to_vec()),
            _ => Err("words say where they stand".into()),
        }
    }

    /// Only the device that orders the stream writes it; what is judged is
    /// that the item is named and the record a JSON object saying where the
    /// words stand, referring to no object.
    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        let name = &after.record.item;
        if name.is_empty() || name.len() > NAME_BYTES {
            return Err("words are named by their notice".into());
        }
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } => {
                Self::decode(body)?;
                let in_object = serde_json::from_slice::<serde_json::Value>(body)
                    .ok()
                    .and_then(|value| value.get("in_object").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false);
                match (in_object, objects.len()) {
                    (false, 0) | (true, 1) => Ok(()),
                    _ => Err("words are their body, or keep their delivery as the one object their body says".into()),
                }
            }
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("words:{scope}")
    }
}
