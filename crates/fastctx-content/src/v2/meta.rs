//! The kind of a conversation's metadata (design §2.4, plan 3-4 group 1):
//! five items in the conversation's scope, beside its timeline — `about`,
//! what it is and where it was born; `place`, the project it is in; `title`,
//! its name, gone while it has none; `options`, the model and effort it
//! starts again with; and `archived`, when it was archived, gone while it
//! is not. Its stream is
//! ordered by the machine the conversation lives on, which alone writes it,
//! made again from the conversation's own directory each time it changes; a
//! body is one JSON object.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// The items a conversation's metadata has.
pub const META_ITEMS: [&str; 5] = ["about", "place", "title", "options", "archived"];

/// A conversation's metadata.
pub struct ConversationMeta;

impl Kind for ConversationMeta {
    /// `conversation.metadata` in the registry (`fastctx_sync::registry`),
    /// whose order only grows.
    const KIND: u16 = 6;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "meta";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &[];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Made again from the conversation's directory.
    const IMPORT: Import = Import::Remade;
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(serde_json::Value::Object(_)) => Ok(body.to_vec()),
            Ok(_) => Err("a conversation's metadata holds one JSON object".into()),
            Err(error) => Err(format!("a conversation's metadata is not JSON: {error}")),
        }
    }

    /// Only the device that orders the stream writes it; what is judged is
    /// that the record is one of the metadata's items, as a JSON object
    /// referring to no object.
    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        if !META_ITEMS.iter().any(|item| item.as_bytes() == after.record.item.as_slice()) {
            return Err("a conversation's metadata has no such item".into());
        }
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } => {
                if !objects.is_empty() {
                    return Err("a conversation's metadata refers to no object".into());
                }
                Self::decode(body).map(|_| ())
            }
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("meta:{scope}")
    }
}
