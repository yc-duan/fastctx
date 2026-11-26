//! A conversation's evidence folders in the asset library (design §28.22,
//! plan 3-4 group 9): each folder the World placed in its `artifacts/` for
//! its goal's judge, with its files as objects — on every device that holds
//! the conversation, and on the hub for good. A folder never changes once
//! placed, so only those the library lacks are written, a few at a time.
//! Looked at by the timeline's job, which placing a folder asks for.

use super::beside::{Beside, writable};
use crate::Node;
use fastctx_content::v2::{ConversationArtifact, Kind as _, LEASE, Write, Writer};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use std::sync::Arc;

/// The most folders, and bytes, one look writes.
const FOLDERS: usize = 16;
const BYTES: u64 = 256 << 20;
/// The most a part's list of files holds, with room for the rest of its
/// body.
const PART_BYTES: usize = fastctx_content::RECORD_BYTES - 4096;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Whether `key` names an evidence folder (`placement::artifacts`).
fn is_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 128 && key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Writes the evidence folders of `resident` its library lacks; how many.
pub(super) async fn produce(
    node: &Arc<Node>,
    resident: &ResidentId,
    producing: &super::serial::Serial,
) -> Result<usize, Failure> {
    let held = producing.hold(resident).await;
    let folder = node.world.conversation(resident).path().join("artifacts");
    if !fastctx_platform::disk::exists(&folder).map_err(failure)? {
        return Ok(0);
    }
    let Some(Beside { library, scope, holding }) =
        writable(node, resident, ConversationArtifact::STREAM, &held).await?
    else {
        return Ok(0);
    };
    let signer = node.assets.as_ref().map(|assets| assets.signer()).ok_or_else(|| failure("no asset library"))?;
    fastctx_platform::task::blocking(move || {
        let _held = held;
        let mut keys: Vec<(String, std::path::PathBuf)> = Vec::new();
        for entry in fastctx_platform::disk::read_dir(&folder).map_err(failure)? {
            let entry = entry.map_err(failure)?;
            let Some(key) = entry.file_name().to_str().map(str::to_string) else { continue };
            if is_key(&key) && entry.file_type().map_err(failure)?.is_dir() {
                keys.push((key, entry.path()));
            }
        }
        keys.sort();
        let (mut writes, mut leases, mut bytes) = (Vec::new(), Vec::new(), 0u64);
        let mut more = false;
        for (key, path) in keys {
            if writes.len() >= FOLDERS || bytes >= BYTES {
                more = true;
                break;
            }
            let item = fastctx_content::v2::item_of(&key);
            if library.item(&scope, ConversationArtifact::STREAM, &item).map_err(failure)?.is_some() {
                continue;
            }
            // Every file of it at any depth, named by its path within it
            // (M20).
            let mut names: Vec<(String, std::path::PathBuf)> = Vec::new();
            let mut folders = vec![(String::new(), path.clone())];
            while let Some((within, folder)) = folders.pop() {
                for entry in fastctx_platform::disk::read_dir(&folder).map_err(failure)? {
                    let entry = entry.map_err(failure)?;
                    let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
                    let named = if within.is_empty() { name } else { format!("{within}/{name}") };
                    let kind = entry.file_type().map_err(failure)?;
                    if kind.is_dir() {
                        folders.push((named, entry.path()));
                    } else if kind.is_file() {
                        names.push((named, entry.path()));
                    }
                }
            }
            names.sort();
            // Every file, in parts of as many as a record refers to and a
            // body holds, written together (PP16, N16).
            let mut parts: Vec<(Vec<serde_json::Value>, Vec<fastctx_content::ObjectRef>)> =
                vec![(Vec::new(), Vec::new())];
            let mut listed = 0usize;
            for (name, file_path) in names {
                let mut file = fastctx_platform::disk::open(&file_path).map_err(failure)?;
                let lease = library.put(&mut file, "artifacts", LEASE).map_err(failure)?;
                let sha256: String = lease.object.hash.iter().map(|byte| format!("{byte:02x}")).collect();
                let entry = serde_json::json!({ "name": name, "size": lease.object.size, "sha256": sha256 });
                let size = entry.to_string().len() + 1;
                let full = parts.last().is_some_and(|(files, _)| files.len() >= fastctx_content::RECORD_OBJECTS)
                    || listed + size > PART_BYTES;
                if full {
                    parts.push((Vec::new(), Vec::new()));
                    listed = 0;
                }
                listed += size;
                let (files, objects) = parts.last_mut().expect("a part");
                files.push(entry);
                bytes += lease.object.size;
                objects.push(lease.object);
                leases.push(lease);
            }
            let count = parts.len() as u64;
            for (part, (files, objects)) in (0u64..).zip(parts) {
                let body = serde_json::to_vec(
                    &serde_json::json!({ "key": key, "part": part, "parts": count, "files": files, "omitted": 0 }),
                )
                .map_err(failure)?;
                let item = fastctx_content::v2::item_of(&fastctx_content::v2::part_name(&key, part));
                writes.push(Write::<ConversationArtifact> { order: item.clone(), item, value: Some(body), objects });
            }
        }
        if writes.is_empty() {
            return Ok((0, more));
        }
        let count = writes.len();
        Writer::<ConversationArtifact>::new(&library, &scope, ConversationArtifact::STREAM)
            .map_err(failure)?
            .in_holding(holding)
            .write_all(writes, leases, false, signer)
            .map_err(failure)?;
        Ok::<_, Failure>((count, more))
    })
    .await
    .map(|(count, more)| {
        // Cut short by its bounds: the rest in the next look (OO23).
        if more {
            super::tail::wanted(node, resident);
        }
        count
    })
}
