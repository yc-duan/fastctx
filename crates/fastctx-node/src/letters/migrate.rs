//! Receipt migration never blocks startup. Each committed batch is verified
//! before its legacy files are removed; uncertain acceptances have no expiry.

use super::delivered;
use crate::Node;
use fastctx_storage::migration;
use std::io;
use std::sync::Arc;

pub(crate) fn start(node: &Arc<Node>) {
    let node = Arc::downgrade(node);
    tokio::spawn(async move {
        let mut previous = String::new();
        let mut again = crate::again::Again::default();
        loop {
            let Some(node) = node.upgrade() else { return };
            let root = node.world.path().join("letter-receipts");
            let database = node.database.clone();
            let result = tokio::task::spawn_blocking(move || -> io::Result<usize> {
                let entries = match std::fs::read_dir(&root) {
                    Ok(entries) => entries,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
                    Err(error) => return Err(error),
                };
                let mut batch = Vec::new();
                let mut count = 0;
                for entry in entries {
                    let entry = entry?;
                    if !entry.file_type()?.is_file() {
                        continue;
                    }
                    let id = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| io::Error::other("letter receipt id is not UTF-8"))?;
                    if id.starts_with('.') {
                        continue;
                    }
                    let bytes = match std::fs::read(entry.path()) {
                        Ok(bytes) => bytes,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error),
                    };
                    if bytes != b"accepted" {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("invalid letter receipt: {}", entry.path().display()),
                        ));
                    }
                    batch.push(migration::Receipt {
                        path: entry.path(),
                        key: delivered::key(&id),
                        value: bytes,
                        keep: delivered::KEEP,
                    });
                    if batch.len() == 256 {
                        count +=
                            migration::receipts(&database, std::mem::take(&mut batch)).map_err(io::Error::other)?;
                    }
                }
                count += migration::receipts(&database, batch).map_err(io::Error::other)?;
                Ok(count)
            })
            .await;
            match result {
                Ok(Ok(count)) => {
                    if count != 0 {
                        node.log.line(format_args!("moved {count} business letter receipts into the database"));
                    }
                    return;
                }
                result => {
                    let reason = match result {
                        Ok(Err(error)) => error.to_string(),
                        Err(error) => error.to_string(),
                        _ => unreachable!(),
                    };
                    if previous != reason {
                        node.log.line(format_args!("moving business letter receipts: {reason}"));
                        previous = reason;
                    }
                }
            }
            drop(node);
            tokio::time::sleep(again.next()).await;
        }
    });
}
