//! Incremental migration visits each source once and retains malformed records.

use super::{KEEP, key};
use crate::link::letters::Receipt;
use crate::link::state::NetDir;
use fastctx_storage::migration;
use std::io;
use std::path::Path;

fn entries(path: &Path) -> io::Result<Option<std::fs::ReadDir>> {
    match std::fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn run(net: &NetDir) -> io::Result<usize> {
    let mut count = 0;
    for (folder, namespace) in [("received-by-key", "transport-key"), ("received", "transport-name")] {
        let root = net.path().join(folder);
        let Some(senders) = entries(&root)? else { continue };
        for sender in senders {
            let sender = sender?;
            if !sender.file_type()?.is_dir() {
                continue;
            }
            let name = sender.file_name().into_string().map_err(|_| io::Error::other("receipt sender is not UTF-8"))?;
            let name = if namespace == "transport-name" {
                name.parse::<fastctx_types::MachineName>().map_or(name.clone(), |name| name.file_name().into_owned())
            } else {
                name
            };
            let mut batch = Vec::new();
            let Some(files) = entries(&sender.path())? else { continue };
            for file in files {
                let file = file?;
                if !file.file_type()?.is_file() {
                    continue;
                }
                let id = file.file_name().into_string().map_err(|_| io::Error::other("receipt id is not UTF-8"))?;
                if id.starts_with('.') {
                    continue;
                }
                let bytes = match std::fs::read(file.path()) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                let receipt: Receipt = serde_json::from_slice(&bytes).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", file.path().display()))
                })?;
                batch.push(migration::Receipt {
                    path: file.path(),
                    key: key(net, namespace, &name, &id),
                    value: serde_json::to_vec(&receipt).map_err(io::Error::other)?,
                    keep: KEEP,
                });
                if batch.len() == 256 {
                    count +=
                        migration::receipts(net.database(), std::mem::take(&mut batch)).map_err(io::Error::other)?;
                }
            }
            count += migration::receipts(net.database(), batch).map_err(io::Error::other)?;
            match std::fs::remove_dir(sender.path()) {
                Ok(()) => {}
                Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty) => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(count)
}
