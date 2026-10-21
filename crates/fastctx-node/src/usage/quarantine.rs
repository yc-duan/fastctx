//! Preserves unreadable committed rows before removing them from the active ledger.
use super::ledger::Entry;
use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;

pub(super) fn read(path: &Path) -> io::Result<Vec<Entry>> {
    let bytes = std::fs::read(path)?;
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut rows = Vec::new();
    let mut kept = Vec::new();
    let mut offset = 0;
    let mut damaged = false;
    for (line, raw) in bytes[..end].split_inclusive(|b| *b == b'\n').enumerate() {
        let content = &raw[..raw.len() - 1];
        if content.is_empty() {
            kept.extend_from_slice(raw);
        } else {
            match serde_json::from_slice(content) {
                Ok(row) => {
                    rows.push(row);
                    kept.extend_from_slice(raw);
                }
                Err(error) => {
                    let directory = path.parent().expect("ledger directory").join("quarantine");
                    fastctx_platform::ensure_private_dir(&directory)?;
                    let day = path.file_stem().and_then(|s| s.to_str()).unwrap_or("ledger");
                    let key = format!("{day}-{:x}.json", Sha256::digest(raw));
                    let record = serde_json::json!({
                        "file": path.file_name(), "line": line + 1, "offset": offset,
                        "error": error.to_string(), "bytes": raw,
                    });
                    let destination = directory.join(key);
                    if !destination.try_exists()? {
                        fastctx_platform::publish(
                            &destination,
                            &serde_json::to_vec(&record)?,
                            fastctx_platform::Permissions::Private,
                        )?;
                    }
                    damaged = true;
                }
            }
        }
        offset += raw.len();
    }
    if damaged {
        // Preserve an uncommitted tail for the append path's torn-write repair.
        kept.extend_from_slice(&bytes[end..]);
        fastctx_platform::publish(path, &kept, fastctx_platform::Permissions::Private)?;
    }
    Ok(rows)
}

pub(super) fn count(path: &Path) -> io::Result<usize> {
    match std::fs::read_dir(path.join("quarantine")) {
        Ok(mut entries) => entries.try_fold(0, |count, entry| {
            Ok(count + usize::from(entry?.path().extension().is_some_and(|s| s == "json")))
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}
