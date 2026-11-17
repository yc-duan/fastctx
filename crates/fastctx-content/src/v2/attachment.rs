//! The kind of the files the user attached to a conversation (design
//! §28.16, plan 3-4 group 2): one item for each attachment kept in the
//! conversation's directory, named by the folder it is kept in. Its body is
//! the attachment's description — name, type, size and SHA-256 — and its one
//! object, the original, is exactly those bytes: on every device that holds
//! the conversation, and on the hub for good. An attachment whose original
//! is kept on another machine — a file sent on from a machine that assists
//! — is described without it. Written by the machine the conversation lives
//! on, which alone orders the stream.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// An attachment of a conversation.
pub struct ConversationAttachment;

/// What an attachment's body says of its original: its size and SHA-256.
fn described(body: &[u8]) -> Result<(u64, String), String> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|error| format!("an attachment's description: {error}"))?;
    let size = value.get("size").and_then(serde_json::Value::as_u64);
    let sha256 = value.get("sha256").and_then(serde_json::Value::as_str);
    match (size, sha256) {
        (Some(size), Some(sha256)) => Ok((size, sha256.to_string())),
        _ => Err("an attachment is described by its size and SHA-256".into()),
    }
}

impl Kind for ConversationAttachment {
    /// `conversation.attachment` in the registry (`fastctx_sync::registry`),
    /// whose order only grows.
    const KIND: u16 = 18;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "attachments";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &["original"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// What the conversation's directory kept before is taken up from it.
    const IMPORT: Import = Import::From("attachments");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        described(body)?;
        Ok(body.to_vec())
    }

    /// Only the device that orders the stream writes it; what is judged is
    /// that the description is whole and its original, when it refers to
    /// one, is the very bytes it describes.
    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } => {
                let (size, sha256) = described(body)?;
                match objects.as_slice() {
                    [] => Ok(()),
                    [original] => {
                        let hash: String = original.hash.iter().map(|byte| format!("{byte:02x}")).collect();
                        if original.size != size || !hash.eq_ignore_ascii_case(&sha256) {
                            return Err("an attachment's original is not the bytes it describes".into());
                        }
                        Ok(())
                    }
                    _ => Err("an attachment has one original".into()),
                }
            }
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("attachments:{scope}")
    }
}
