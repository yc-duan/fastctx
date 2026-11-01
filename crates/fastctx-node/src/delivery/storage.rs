//! Delivery sources declare every input, including the separate acceptance
//! receipt. Verified originals are archived only after their rows commit.
use super::*;
use crate::records::{self, Source};
use std::path::Path;

pub(super) fn initialize(directory: &ConversationDir) -> io::Result<()> {
    let folder = directory.handovers();
    records::ensure_each(directory, "handovers", move |push| {
        files(&folder, push, |path, push| {
            let mut row: Handover = read(path)?;
            let accepted = path.with_extension("accepted");
            crate::record_migration::file(&accepted, push, |_| {
                let turn: String = read(&accepted)?;
                if row.turn.as_ref().is_some_and(|prior| prior != &turn) {
                    return Err(io::Error::other("a handover and its receipt disagree about their turn"));
                }
                row.turn = Some(turn);
                Ok(())
            })?;
            push(Source::new(row.id.to_string(), path.to_path_buf(), serde_json::to_vec(&row)?)?)
        })
    })?;
    let folder = directory.turns();
    records::ensure_each(directory, "terminals", move |push| {
        files(&folder, push, |path, push| {
            let row: Finished = read(path)?;
            push(Source::new(row.turn.clone(), path.to_path_buf(), serde_json::to_vec(&row)?)?)
        })
    })?;
    records::ensure_file(directory, "terminal-counter", directory.turns().join("order"), |path, push| {
        let order: u64 = read(path)?;
        push(Source::new("order".into(), path.to_path_buf(), serde_json::to_vec(&order)?)?)
    })?;
    let folder = directory.turn_sessions();
    records::ensure_each(directory, "turn-sessions", move |push| {
        files(&folder, push, |path, push| {
            let value: String = read(path)?;
            let key = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| io::Error::other("invalid turn receipt"))?;
            push(Source::new(key.into(), path.to_path_buf(), serde_json::to_vec(&value)?)?)
        })
    })?;
    let source = directory.clone();
    records::ensure_step(directory, "handover-index", move || {
        let mut after = 0;
        loop {
            let page = records::read(&source, |rows| rows.page::<Handover>("handovers", after, 16))?;
            let Some(last) = page.last() else { break };
            after = last.0;
            for (_, row) in page {
                for inputs in row.inputs.chunks(16) {
                    let (key, inputs) = (row.id.clone(), inputs.to_vec());
                    records::write_at(&source, crate::record_migration::scheduler::priority(), 1024, move |rows| {
                        for input in inputs {
                            rows.link("handovers", input.delivery.id.as_str(), key.as_str())?;
                        }
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    })?;
    super::migrate_terminals::run(directory)
}

fn files(
    directory: &Path,
    push: &mut dyn FnMut(Source) -> io::Result<()>,
    mut apply: impl FnMut(&Path, &mut dyn FnMut(Source) -> io::Result<()>) -> io::Result<()>,
) -> io::Result<()> {
    let entries = match fastctx_platform::disk::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "json")
            && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            crate::record_migration::file(&path, push, |push| apply(&path, push))?;
        }
    }
    Ok(())
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&fastctx_platform::disk::read(path)?).map_err(io::Error::other)
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
