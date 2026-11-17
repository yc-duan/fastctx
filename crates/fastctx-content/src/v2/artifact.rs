//! A conversation's evidence folders (design §28.22, plan 3-4 group 9): the
//! folders the World placed in its `artifacts/` for its goal's judge, each
//! by its key. Its body names each file the folder holds with its size and
//! SHA-256, and its objects are those files, in that order: on every device
//! that holds the conversation, and on the hub for good. A folder, once
//! placed, never changes; it goes only with its conversation. Written by the
//! machine the conversation lives on, which alone orders the stream. A
//! folder of more files than a record refers to, or whose list is longer
//! than a record's body, is several records, its parts, written together.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// The evidence folders of a conversation.
pub struct ConversationArtifact;

/// A file a folder's body names: its name, size and SHA-256.
type Listed = (String, u64, String);

/// The name of part `part` of the folder `key`: the key itself for the
/// first, the key and the part's number after a `/` — which no folder's key
/// holds — for the rest (PP16, N16).
pub fn part_name(key: &str, part: u64) -> String {
    if part == 0 { key.to_string() } else { format!("{key}/{part}") }
}

/// The key a folder's body says, with its part, and the files it names.
fn files(body: &[u8]) -> Result<(String, Vec<Listed>), String> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|error| format!("an evidence folder: {error}"))?;
    let key = value.get("key").and_then(serde_json::Value::as_str).ok_or("an evidence folder says its key")?;
    let part = value.get("part").and_then(serde_json::Value::as_u64).unwrap_or(0);
    let key = part_name(key, part);
    let listed = value.get("files").and_then(serde_json::Value::as_array).ok_or("an evidence folder lists its files")?;
    let mut files = Vec::with_capacity(listed.len());
    for file in listed {
        let name = file.get("name").and_then(serde_json::Value::as_str);
        let size = file.get("size").and_then(serde_json::Value::as_u64);
        let sha256 = file.get("sha256").and_then(serde_json::Value::as_str);
        let (Some(name), Some(size), Some(sha256)) = (name, size, sha256) else {
            return Err("each file of an evidence folder has its name, size and SHA-256".into());
        };
        files.push((name.to_string(), size, sha256.to_string()));
    }
    Ok((key.to_string(), files))
}

impl Kind for ConversationArtifact {
    /// `conversation.artifact` in the registry, whose order only grows.
    const KIND: u16 = 17;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "artifacts";
    const HOLDING: Holding = Holding::Readers;
    /// Each object is one of the folder's files, as many as a record
    /// refers to.
    const ROLES: &'static [&'static str] = &["file"; crate::record::RECORD_OBJECTS];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the conversation's `artifacts/`.
    const IMPORT: Import = Import::From("artifacts");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        files(body)?;
        Ok(body.to_vec())
    }

    /// A folder is named by its key and its objects are the very files it
    /// lists; once written it stays as it is.
    fn authorize(_actor: &Actor<'_>, before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        // Let go only with its conversation, never on its own: one let go
        // could be placed again otherwise (AS15).
        let State::Is { body, objects } = &after.record.state else {
            return Err("an evidence folder goes only with its conversation".into());
        };
        if let Some(before) = before {
            return if before.body == *body && before.objects == *objects && before.order == after.record.order {
                Ok(())
            } else {
                Err("an evidence folder, once placed, stays as it is".into())
            };
        }
        let (key, files) = files(body)?;
        if super::names::item_of(&key) != after.record.item {
            return Err("an evidence folder's part is named by its key and its number".into());
        }
        if files.len() != objects.len() {
            return Err("an evidence folder's objects are the files it lists".into());
        }
        for ((_, size, sha256), object) in files.iter().zip(objects) {
            let hash: String = object.hash.iter().map(|byte| format!("{byte:02x}")).collect();
            if object.size != *size || !hash.eq_ignore_ascii_case(sha256) {
                return Err("an evidence folder's file is not the bytes it lists".into());
            }
        }
        Ok(())
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("artifacts:{scope}")
    }
}
