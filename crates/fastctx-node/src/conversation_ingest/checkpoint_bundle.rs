//! Checkpoint JSON names directory entries, never private content hashes. The
//! manifest's explicit edges retain every dependency for the source-state reader.
use fastctx_hosts::claude::ingest::checkpoint::ClaudeCheckpointV1;
use fastctx_sync::{
    identity::{Digest, ObjectId},
    manifest::{
        ContentRef, DirectoryEntry, FileEntry, ManifestPage, ManifestReply, ManifestRoot, PathName, RelativePath,
    },
};
use fastctx_types::sync::{cursor::ByteCount, transport_codec::bounded_json};
use std::{collections::BTreeMap, io};

const LINKS: [(&str, &str); 10] = [
    ("/read_ahead/content", "ahead"),
    ("/anchors_root", "anchors"),
    ("/row_index_root", "rows"),
    ("/partial/line/document/bodies/raw/content", "raw"),
    ("/partial/line/document/bodies/strings/content", "strings"),
    ("/partial/line/document/bodies/nodes/content", "nodes"),
    ("/partial/row/content/bodies/raw/content", "raw"),
    ("/partial/row/content/bodies/strings/content", "strings"),
    ("/partial/row/content/bodies/nodes/content", "nodes"),
    ("/partial/consumer", "consumer"),
];
pub struct StateBundle {
    json: Vec<u8>,
    entries: Vec<DirectoryEntry>,
    checkpoint: ClaudeCheckpointV1,
}
pub fn encode(checkpoint: &ClaudeCheckpointV1) -> io::Result<StateBundle> {
    let bytes = bounded_json(checkpoint, 64 * 1024).map_err(io::Error::other)?;
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let mut entries = Vec::with_capacity(8);
    for (pointer, name) in LINKS {
        if let Some(field) = value.pointer_mut(pointer)
            && !field.is_null()
        {
            let content: ContentRef = serde_json::from_value(field.clone()).map_err(io::Error::other)?;
            entries.push(entry(name, content));
            *field = serde_json::Value::String(name.into());
        }
    }
    let json = bounded_json(&value, 64 * 1024).map_err(io::Error::other)?;
    Ok(StateBundle { json, entries, checkpoint: checkpoint.clone() })
}
pub fn decode(json: &[u8], entries: &[DirectoryEntry]) -> io::Result<ClaudeCheckpointV1> {
    if json.len() > 64 * 1024 || entries.len() > 8 {
        return Err(io::Error::other("checkpoint state bundle exceeds its bound"));
    }
    let mut links = BTreeMap::new();
    for entry in entries {
        let (path, content) = match entry {
            DirectoryEntry::File { file } => (&file.path, file.content.clone()),
            DirectoryEntry::Directory { path, root: Some(root) } => (path, ContentRef::Manifest { root: root.clone() }),
            _ => return Err(io::Error::other("unexpected checkpoint directory entry")),
        };
        let [PathName::Utf8(name)] = path.components.as_slice() else {
            return Err(io::Error::other("checkpoint entry has an invalid path"));
        };
        if links.insert(name.as_str(), content).is_some() {
            return Err(io::Error::other("duplicate checkpoint entry"));
        }
    }
    let mut value: serde_json::Value = serde_json::from_slice(json).map_err(io::Error::other)?;
    for (pointer, name) in LINKS {
        if let Some(field) = value.pointer_mut(pointer)
            && !field.is_null()
        {
            if field.as_str() != Some(name) {
                return Err(io::Error::other("checkpoint reference is not its declared entry path"));
            }
            let content = links.get(name).ok_or_else(|| io::Error::other("checkpoint dependency entry is absent"))?;
            *field = serde_json::to_value(content).map_err(io::Error::other)?;
        }
    }
    serde_json::from_value(value).map_err(io::Error::other)
}
pub fn entry(name: &str, content: ContentRef) -> DirectoryEntry {
    let path = RelativePath { components: vec![PathName::Utf8(name.into())] };
    match content {
        ContentRef::Manifest { root } => DirectoryEntry::Directory { path, root: Some(root) },
        ContentRef::Single { decoded_bytes, .. } => DirectoryEntry::File {
            file: FileEntry {
                object: ObjectId::from_bytes(
                    Digest::of(format!("fastctx.claude.state-entry.v1\0{name}").as_bytes()).bytes(),
                ),
                path,
                bytes: ByteCount(u64::from(decoded_bytes)),
                executable: false,
                content,
            },
        },
    }
}
/// A checked binding between private parser state and the exact directory bytes
/// submitted to the block store. Construction never grants custody or read rights.
pub struct BoundState {
    checkpoint: ClaudeCheckpointV1,
    root: ManifestRoot,
}
impl BoundState {
    pub fn checkpoint(&self) -> &ClaudeCheckpointV1 {
        &self.checkpoint
    }
    pub fn content(&self) -> ContentRef {
        ContentRef::Manifest { root: self.root.clone() }
    }
    pub fn restore(reply: &ManifestReply, json: &[u8]) -> io::Result<Self> {
        let entries = verified_directory(reply)?;
        let checkpoint = decode(json, &entries)?;
        encode(&checkpoint)?.bind(reply)
    }
}
impl StateBundle {
    pub fn json(&self) -> &[u8] {
        &self.json
    }

    /// `state_json` was uploaded with the same operation as the enclosing ingest.
    pub fn directory(&self, state_json: ContentRef) -> io::Result<ManifestPage> {
        match &state_json {
            ContentRef::Single { decoded_bytes, .. }
                if *decoded_bytes as usize == self.json.len() && state_json.id().digest == Digest::of(&self.json) => {}
            _ => return Err(io::Error::other("checkpoint state.json differs from its uploaded bytes")),
        }
        let mut entries = self.entries.clone();
        entries.push(entry("state.json", state_json));
        Ok(ManifestPage::Directory { entries })
    }
    /// Bind the exact manifest submitted to put_manifest, not an independent root
    /// supplied beside a checkpoint. The engine still checks storage and custody.
    pub fn bind(self, reply: &ManifestReply) -> io::Result<BoundState> {
        let entries = verified_directory(reply)?;
        let state_json = entries
            .iter()
            .find_map(|entry| match entry {
                DirectoryEntry::File { file } if file.path.components == [PathName::Utf8("state.json".into())] => {
                    Some(file.content.clone())
                }
                _ => None,
            })
            .ok_or_else(|| io::Error::other("checkpoint state.json entry is absent"))?;
        if self.directory(state_json)? != (ManifestPage::Directory { entries }) {
            return Err(io::Error::other("checkpoint differs from its persisted directory"));
        }
        Ok(BoundState { checkpoint: self.checkpoint, root: reply.root.clone() })
    }
}
fn verified_directory(reply: &ManifestReply) -> io::Result<Vec<DirectoryEntry>> {
    if reply.page_json.len() > 64 * 1024 || Digest::of(reply.page_json.as_bytes()) != reply.root.0.digest {
        return Err(io::Error::other("checkpoint directory digest or length differs"));
    }
    match serde_json::from_str(&reply.page_json).map_err(io::Error::other)? {
        ManifestPage::Directory { entries } if entries.len() <= 8 => Ok(entries),
        _ => Err(io::Error::other("checkpoint state requires its bounded directory")),
    }
}
