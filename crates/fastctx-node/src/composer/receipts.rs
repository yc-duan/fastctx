//! Indexed operation lookup, with dual reads while legacy files are migrating.
//! Age alone never proves that a device cannot resend an operation.

use super::*;
use fastctx_storage::rusqlite::{OptionalExtension, params};
use serde::de::DeserializeOwned;

pub(super) fn name(key: &str) -> String {
    crate::speech::fixed_id("operation", &[key]).to_string()
}

async fn get<T: DeserializeOwned + Send + 'static>(
    node: &Node,
    id: &ResidentId,
    kind: &'static str,
    key: &str,
) -> Result<Option<T>, Failure> {
    if !node.residents.knows(id) {
        return Err(failure("unknown resident"));
    }
    let (owner, name) = (id.to_string(), name(key));
    let directory = node.world.conversation(id);
    node.database
        .read(move |connection| {
            let query =
                |connection: &fastctx_storage::rusqlite::Connection| -> fastctx_storage::Result<Option<Vec<u8>>> {
                    Ok(connection
                        .query_row(
                            "SELECT content FROM composer_records WHERE owner=?1 AND kind=?2 AND name=?3",
                            params![owner, kind, name],
                            |row| row.get(0),
                        )
                        .optional()?)
                };
            let bytes = match query(connection)? {
                Some(bytes) => Some(bytes),
                None => {
                    let legacy = {
                        let _transition = crate::resident_storage::lock();
                        std::fs::read(directory.composer_books(kind).join(format!("{name}.json")))
                    };
                    match legacy {
                        Ok(bytes) => Some(bytes),
                        Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => query(connection)?,
                        Err(reason) => return Err(fastctx_storage::Error::new(reason.to_string())),
                    }
                }
            };
            bytes
                .map(|bytes| {
                    serde_json::from_slice(&bytes).map_err(|reason| fastctx_storage::Error::new(reason.to_string()))
                })
                .transpose()
        })
        .await
        .map_err(failure)
}

pub(super) async fn receipt(
    node: &Node,
    id: &ResidentId,
    state: &Stored,
    operation: &str,
) -> Result<Option<Receipt>, Failure> {
    match state.receipts.get(operation) {
        Some(receipt) => Ok(Some(receipt.clone())),
        None => get(node, id, "receipts", operation).await,
    }
}
pub(super) async fn upload(
    node: &Node,
    id: &ResidentId,
    state: &Stored,
    operation: &str,
) -> Result<Option<uploads::UploadRecord>, Failure> {
    let mut record = match state.uploads.get(operation) {
        Some(receipt) => Some(receipt.clone()),
        None => get(node, id, "uploads", operation).await?,
    };
    if let Some(record) = &mut record {
        uploads::interrupted(node, id, operation, record);
    }
    Ok(record)
}
pub(super) async fn ended(node: &Node, id: &ResidentId, state: &Stored, turn: &str) -> Result<bool, Failure> {
    Ok(state.ended.contains(turn) || get::<bool>(node, id, "ended", turn).await?.unwrap_or(false))
}
