//! The kind of the files a conversation changed (`gui-files`): one item for
//! each change of one file, in the conversation's scope beside its timeline,
//! named by what made it — an edit its host made (a tool call of the turn,
//! and which of its changes) or a file one of the World's runs wrote (the
//! write's own id). Its body is one JSON object: where and what the file
//! is, how it changed, by how many lines when that is known, in which turn
//! and call, and when. Written by the machine the conversation lives on,
//! which alone orders the stream, from its timeline and its runs' facts; the
//! lines of a change are read where they are kept, not here.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// How a file can have changed.
pub const FILE_HOWS: [&str; 3] = ["added", "changed", "removed"];

/// The longest name an item of file changes has.
const NAME_BYTES: usize = 256;

/// The longest body a file change has: it holds no lines.
const BODY_BYTES: usize = 16 * 1024;

/// A change of a file a conversation made.
pub struct ConversationFileChange;

impl Kind for ConversationFileChange {
    /// `conversation.files` in the registry (`fastctx_sync::registry`),
    /// whose order only grows.
    const KIND: u16 = 56;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "files";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &[];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Made again from the timeline and the runs' facts it is made of.
    const IMPORT: Import = Import::Remade;
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        if body.len() > BODY_BYTES {
            return Err("a file change is a short record".into());
        }
        let value: serde_json::Value =
            serde_json::from_slice(body).map_err(|error| format!("a file change is not JSON: {error}"))?;
        let path = value.get("path").and_then(serde_json::Value::as_str);
        let how = value.get("how").and_then(serde_json::Value::as_str);
        match (path, how) {
            (Some(path), Some(how)) if !path.is_empty() && FILE_HOWS.contains(&how) => Ok(body.to_vec()),
            _ => Err("a file change names its file and how it changed".into()),
        }
    }

    /// Only the device that orders the stream writes it; what is judged is
    /// that the item is named and the record whole, referring to no object.
    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        let name = &after.record.item;
        if name.is_empty() || name.len() > NAME_BYTES {
            return Err("a file change is named by what made it".into());
        }
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } => {
                Self::decode(body)?;
                if !objects.is_empty() {
                    return Err("a file change refers to no object".into());
                }
                Ok(())
            }
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("files:{scope}")
    }
}
