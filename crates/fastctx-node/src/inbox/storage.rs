//! Individually indexed incoming words, answers and durable dispositions.
//! Unconsumed work has no expiry; receipts remain while its source can replay.

use super::*;
use crate::records::{self, Source};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Receipt {
    Heard,
    LegacyDelivered,
    Consumed,
    Answered,
}

pub(crate) fn initialize(directory: &ConversationDir) -> io::Result<()> {
    for (family, folder) in [("pending-inputs", directory.inbox()), ("uncertain-inputs", directory.uncertain())] {
        records::ensure_each(directory, family, |push| {
            json_files(&folder, |path| {
                let row = super::parse_pending(directory, path, &std::fs::read(path)?)?;
                push(
                    Source::new(row.delivery.id.to_string(), path.to_path_buf(), serde_json::to_vec(&row)?)?
                        .ordered(row.delivery.at),
                )
            })
        })?;
    }
    records::ensure_each(directory, "delivery-receipts", |push| {
        for path in entries(&directory.delivery_receipts())? {
            if !path.is_file() || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')) {
                continue;
            }
            let key = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| io::Error::other("invalid receipt"))?
                .to_string();
            let row = match std::fs::read(&path)?.as_slice() {
                b"" => Receipt::Heard,
                b"consumed" => Receipt::Consumed,
                b"answered" => Receipt::Answered,
                _ => return Err(io::Error::other("unknown delivery receipt")),
            };
            push(Source::new(key, path, serde_json::to_vec(&row)?)?)?;
        }
        let path = directory.delivered();
        match std::fs::File::open(&path) {
            Ok(file) => {
                for line in io::BufReader::new(file).lines() {
                    let key = line?;
                    if !key.is_empty() && !directory.delivery_receipts().join(&key).try_exists()? {
                        push(Source::new(key, path.clone(), serde_json::to_vec(&Receipt::LegacyDelivered)?)?)?;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(())
    })?;
    records::ensure_each(directory, "outgoing-answers", |push| {
        json_files(&directory.outbox(), |path| {
            let row: Owing = serde_json::from_slice(&std::fs::read(path)?)?;
            push(
                Source::new(row.delivery.id.to_string(), path.to_path_buf(), serde_json::to_vec(&row)?)?
                    .ordered(row.delivery.at),
            )
        })
    })?;
    records::ensure_each(directory, "paid-answers", |push| {
        for path in entries(&directory.paid())?.into_iter().chain(
            entries(&directory.outbox())?.into_iter().filter(|path| path.extension().is_some_and(|ext| ext == "paid")),
        ) {
            if !path.is_file() || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')) {
                continue;
            }
            let key =
                if path.parent() == Some(directory.outbox().as_path()) { path.file_stem() } else { path.file_name() }
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| io::Error::other("invalid paid receipt"))?
                    .to_string();
            if path.parent() == Some(directory.outbox().as_path()) && directory.paid().join(&key).try_exists()? {
                continue;
            }
            push(Source::new(key, path, b"true".to_vec())?)?;
        }
        Ok(())
    })?;
    records::ensure_each(directory, "undeliverable-answers", |push| {
        for path in entries(&directory.book("undeliverable"))? {
            if !path.is_file() || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')) {
                continue;
            }
            let row: fastctx_types::Address = serde_json::from_slice(&std::fs::read(&path)?)?;
            let key = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| io::Error::other("invalid undeliverable receipt"))?
                .to_string();
            push(Source::new(key, path, serde_json::to_vec(&row)?)?)?;
        }
        Ok(())
    })
}
fn json_files(folder: &Path, mut apply: impl FnMut(&Path) -> io::Result<()>) -> io::Result<()> {
    for path in entries(folder)? {
        if path.extension().is_some_and(|extension| extension == "json")
            && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            apply(&path)?;
        }
    }
    Ok(())
}
pub(crate) fn read_rows<T>(
    directory: &ConversationDir,
    read: impl FnOnce(&records::Rows<'_>) -> fastctx_storage::Result<T>,
) -> io::Result<T> {
    initialize(directory)?;
    records::read(directory, read)
}
pub(crate) fn change<T: Send + 'static>(
    directory: &ConversationDir,
    bytes: usize,
    write: impl FnOnce(&records::Rows<'_>) -> fastctx_storage::Result<T> + Send + 'static,
) -> io::Result<T> {
    initialize(directory)?;
    records::write_at(directory, fastctx_storage::Priority::Interactive, bytes, write)
}
pub(crate) fn delivered(rows: &records::Rows<'_>, key: &str) -> fastctx_storage::Result<bool> {
    Ok(rows.get::<Receipt>("delivery-receipts", key)?.is_some())
}
pub(crate) fn paid(rows: &records::Rows<'_>, key: &str) -> fastctx_storage::Result<bool> {
    Ok(rows.get::<bool>("paid-answers", key)?.is_some()
        || rows.get::<fastctx_types::Address>("undeliverable-answers", key)?.is_some())
}
pub(super) fn pending(rows: &records::Rows<'_>) -> fastctx_storage::Result<Vec<Pending>> {
    rows.unsettled("pending-inputs", "delivery-receipts", "uncertain-inputs")
}
