//! Migration of delivery families. Acceptance receipts are merged before the
//! handover is exposed; the original intent's write time remains its provenance.

use super::*;
use crate::records::{self, Source};
use std::path::Path;

pub(super) fn initialize(directory: &ConversationDir) -> io::Result<()> {
    records::ensure_each(directory, "handovers", |push| {
        files(&directory.handovers(), |path| {
            let mut row: Handover = read(path)?;
            if let Some(turn) = optional::<String>(&path.with_extension("accepted"))? {
                if row.turn.as_ref().is_some_and(|prior| prior != &turn) {
                    return Err(io::Error::other("a handover and its receipt disagree about their turn"));
                }
                row.turn = Some(turn);
            }
            push(Source::new(row.id.to_string(), path.to_path_buf(), serde_json::to_vec(&row)?)?)
        })
    })?;
    records::ensure_each(directory, "terminals", |push| {
        files(&directory.turns(), |path| {
            let row: Finished = read(path)?;
            push(Source::new(row.turn.clone(), path.to_path_buf(), serde_json::to_vec(&row)?)?)
        })
    })?;
    records::ensure_each(directory, "terminal-counter", |push| {
        let path = directory.turns().join("order");
        if let Some(order) = optional::<u64>(&path)? {
            push(Source::new("order".into(), path, serde_json::to_vec(&order)?)?)?;
        }
        Ok(())
    })?;
    records::ensure_each(directory, "turn-sessions", |push| {
        files(&directory.turn_sessions(), |path| {
            let value: String = read(path)?;
            let key = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| io::Error::other("invalid turn receipt"))?;
            push(Source::new(key.into(), path.to_path_buf(), serde_json::to_vec(&value)?)?)
        })
    })?;
    records::ensure_each(directory, "handover-index", |_| {
        let mut after = 0;
        loop {
            let page = records::read(directory, |rows| rows.page::<Handover>("handovers", after, 16))?;
            let Some(last) = page.last() else { break };
            after = last.0;
            for (_, row) in page {
                for inputs in row.inputs.chunks(16) {
                    let (key, inputs) = (row.id.clone(), inputs.to_vec());
                    records::write_at(directory, fastctx_storage::Priority::Background, 1024, move |rows| {
                        for input in inputs {
                            rows.link("handovers", input.delivery.id.as_str(), key.as_str())?;
                        }
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    })
}

fn files(directory: &Path, mut apply: impl FnMut(&Path) -> io::Result<()>) -> io::Result<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "json")
            && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            apply(&path)?;
        }
    }
    Ok(())
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)
}
fn optional<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match read(path) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(super) fn read_rows<T>(
    directory: &ConversationDir,
    read: impl FnOnce(&records::Rows<'_>) -> fastctx_storage::Result<T>,
) -> io::Result<T> {
    initialize(directory)?;
    records::read(directory, read)
}
pub(super) fn change<T: Send + 'static>(
    directory: &ConversationDir,
    bytes: usize,
    write: impl FnOnce(&records::Rows<'_>) -> fastctx_storage::Result<T> + Send + 'static,
) -> io::Result<T> {
    initialize(directory)?;
    records::write_at(directory, fastctx_storage::Priority::Interactive, bytes, write)
}
