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
    let Some(Beside { library, scope, holding }) = writable(node, resident, ConversationArtifact::STREAM, &held).await?
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
        for (key, path) in keys {
            if writes.len() >= FOLDERS || bytes >= BYTES {
                break;
            }
            let item = key.clone().into_bytes();
            if library.item(&scope, ConversationArtifact::STREAM, &item).map_err(failure)?.is_some() {
                continue;
            }
            let mut names: Vec<(String, std::path::PathBuf)> = Vec::new();
            for entry in fastctx_platform::disk::read_dir(&path).map_err(failure)? {
                let entry = entry.map_err(failure)?;
                if entry.file_type().map_err(failure)?.is_file()
                    && let Some(name) = entry.file_name().to_str()
                {
                    names.push((name.to_string(), entry.path()));
                }
            }
            names.sort();
            // As many files as a record refers to; the rest said so.
            let omitted = names.len().saturating_sub(fastctx_content::RECORD_OBJECTS);
            names.truncate(fastctx_content::RECORD_OBJECTS);
            let (mut files, mut objects) = (Vec::new(), Vec::new());
            for (name, file_path) in names {
                let mut file = fastctx_platform::disk::open(&file_path).map_err(failure)?;
                let lease = library.put(&mut file, "artifacts", LEASE).map_err(failure)?;
                let sha256: String = lease.object.hash.iter().map(|byte| format!("{byte:02x}")).collect();
                files.push(serde_json::json!({ "name": name, "size": lease.object.size, "sha256": sha256 }));
                bytes += lease.object.size;
                objects.push(lease.object);
                leases.push(lease);
            }
            let body = serde_json::to_vec(&serde_json::json!({ "key": key, "files": files, "omitted": omitted }))
                .map_err(failure)?;
            writes.push(Write::<ConversationArtifact> { order: item.clone(), item, value: Some(body), objects });
        }
        if writes.is_empty() {
            return Ok(0);
        }
        let count = writes.len();
        Writer::<ConversationArtifact>::new(&library, &scope, ConversationArtifact::STREAM)
            .map_err(failure)?
            .in_holding(holding)
            .write_all(writes, leases, false, signer)
            .map_err(failure)?;
        Ok::<_, Failure>(count)
    })
    .await
}
