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
        let source = directory.clone();
        records::ensure_each(directory, family, move |push| {
            json_files(&folder, push, |path, push| {
                let bytes = fastctx_platform::disk::read(path)?;
                let row = super::parse_pending(&source, path, &bytes)?;
                push(
                    Source::new(
                        row.delivery.id.to_string(),
                        path.to_path_buf(),
                        records::legacy::raw::merge(&bytes, &row)?,
                    )?
                    .ordered(row.delivery.at),
                )
            })
        })?;
    }
    let source = directory.clone();
    records::ensure_each(directory, "delivery-receipts", move |push| {
        for path in entries(&source.delivery_receipts())? {
            if !fastctx_platform::disk::is_file(&path)
                || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            crate::record_migration::file(&path, push, |push| {
                let key = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| crate::record_migration::errors::invalid("invalid receipt"))?
                    .to_string();
                let row = match fastctx_platform::disk::read(&path)?.as_slice() {
                    b"" => Receipt::Heard,
                    b"consumed" => Receipt::Consumed,
                    b"answered" => Receipt::Answered,
                    _ => return Err(crate::record_migration::errors::invalid("unknown delivery receipt")),
                };
                push(Source::new(key, path.clone(), serde_json::to_vec(&row)?)?)
            })?;
        }
        let path = source.delivered();
        crate::record_migration::file(&path, push, |push| {
            for line in io::BufReader::new(fastctx_platform::disk::open(&path)?).lines() {
                let key = line?;
                if key.is_empty() {
                    continue;
                }
                let existing = records::read(&source, |rows| rows.get::<Receipt>("delivery-receipts", &key))?;
                let row = if existing.is_some_and(|receipt| receipt != Receipt::LegacyDelivered) {
                    Source::skip(path.clone())?
                } else {
                    Source::new(key, path.clone(), serde_json::to_vec(&Receipt::LegacyDelivered)?)?
                };
                push(row)?;
            }
            Ok(())
        })
    })?;
    let folder = directory.outbox();
    records::ensure_each(directory, "outgoing-answers", move |push| {
        json_files(&folder, push, |path, push| {
            let bytes = fastctx_platform::disk::read(path)?;
            let row: Owing = serde_json::from_slice(&bytes)?;
            push(
                Source::new(
                    row.delivery.id.to_string(),
                    path.to_path_buf(),
                    records::legacy::raw::merge(&bytes, &row)?,
                )?
                .ordered(row.delivery.at),
            )
        })
    })?;
    let source = directory.clone();
    records::ensure_each(directory, "paid-answers", move |push| {
        for path in entries(&source.paid())?.into_iter().chain(
            entries(&source.outbox())?.into_iter().filter(|path| path.extension().is_some_and(|ext| ext == "paid")),
        ) {
            if !fastctx_platform::disk::is_file(&path)
                || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            crate::record_migration::file(&path, push, |push| {
                let key =
                    if path.parent() == Some(source.outbox().as_path()) { path.file_stem() } else { path.file_name() }
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| crate::record_migration::errors::invalid("invalid paid receipt"))?
                        .to_string();
                push(Source::new(key, path.clone(), b"true".to_vec())?)
            })?;
        }
        Ok(())
    })?;
    let folder = directory.book("undeliverable");
    records::ensure_each(directory, "undeliverable-answers", move |push| {
        for path in entries(&folder)? {
            if !fastctx_platform::disk::is_file(&path)
                || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            crate::record_migration::file(&path, push, |push| {
                let row: fastctx_types::Address = serde_json::from_slice(&fastctx_platform::disk::read(&path)?)?;
                let key = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| crate::record_migration::errors::invalid("invalid undeliverable receipt"))?
                    .to_string();
                push(Source::new(key, path.clone(), serde_json::to_vec(&row)?)?)
            })?;
        }
        Ok(())
    })
}
fn json_files(
    folder: &Path,
    push: &mut dyn FnMut(Source) -> io::Result<()>,
    mut apply: impl FnMut(&Path, &mut dyn FnMut(Source) -> io::Result<()>) -> io::Result<()>,
) -> io::Result<()> {
    for path in entries(folder)? {
        if path.extension().is_some_and(|extension| extension == "json")
            && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            crate::record_migration::file(&path, push, |push| apply(&path, push))?;
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
