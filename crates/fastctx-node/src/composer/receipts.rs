//! Idempotency outlives an interface. Only current operations belong in RAM;
//! acknowledged checkpoints move their old receipts to individually addressed files.

use super::*;
use fastctx_types::layout::ConversationDir;
use serde::{Serialize, de::DeserializeOwned};

fn path(directory: &ConversationDir, kind: &str, key: &str) -> std::path::PathBuf {
    directory.composer_books(kind).join(format!("{}.json", crate::speech::fixed_id("operation", &[key])))
}

fn put(directory: &ConversationDir, kind: &str, key: &str, value: &impl Serialize) -> Result<(), Failure> {
    let path = path(directory, kind, key);
    fastctx_platform::ensure_private_dir(path.parent().expect("receipt directory")).map_err(failure)?;
    fastctx_platform::publish(
        &path,
        &serde_json::to_vec(value).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)
}

fn get<T: DeserializeOwned>(node: &Node, id: &ResidentId, kind: &str, key: &str) -> Result<Option<T>, Failure> {
    if !node.residents.knows(id) {
        return Err(failure("unknown resident"));
    }
    let directory = node.world.conversation(id);
    let _transition = crate::resident_storage::lock();
    match std::fs::read(path(&directory, kind, key)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(error)),
    }
}

pub(super) fn receipt(
    node: &Node,
    id: &ResidentId,
    state: &Stored,
    operation: &str,
) -> Result<Option<Receipt>, Failure> {
    match state.receipts.get(operation) {
        Some(receipt) => Ok(Some(receipt.clone())),
        None => get(node, id, "receipts", operation),
    }
}

pub(super) fn upload(
    node: &Node,
    id: &ResidentId,
    state: &Stored,
    operation: &str,
) -> Result<Option<uploads::UploadRecord>, Failure> {
    match state.uploads.get(operation) {
        Some(receipt) => Ok(Some(receipt.clone())),
        None => get(node, id, "uploads", operation),
    }
}

pub(super) fn ended(node: &Node, id: &ResidentId, state: &Stored, turn: &str) -> Result<bool, Failure> {
    Ok(state.ended.contains(turn) || get::<bool>(node, id, "ended", turn)?.unwrap_or(false))
}

/// `previous` already has a committed checkpoint. Archive it before removing
/// identical entries from its successor; a failed successor never invents a receipt.
pub(super) fn compact(root: &ConversationDir, previous: &Stored, next: &mut Stored) -> Result<(), Failure> {
    for (key, receipt) in &previous.receipts {
        put(root, "receipts", key, receipt)?;
        if next.receipts.get(key) == Some(receipt) {
            next.receipts.remove(key);
        }
    }
    for turn in &previous.ended {
        put(root, "ended", turn, &true)?;
        next.ended.remove(turn);
    }
    for (key, record) in &previous.uploads {
        if record.finished() {
            put(root, "uploads", key, record)?;
            if next.uploads.get(key) == Some(record) {
                next.uploads.remove(key);
            }
        }
    }
    Ok(())
}
