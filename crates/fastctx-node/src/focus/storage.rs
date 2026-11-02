//! Focus keeps three current values and one render generation per conversation.
//! Each value is at most 4 MiB, staged in bounded payload pieces; replacements
//! release their old roots to payload collection. No focus history is retained.
//! The opening survives until conversation deletion. A generation prevents a
//! cancelled or slow render from replacing a newer authorized selection.
use super::Opening;
use crate::{
    record_value::{Prepared, Value},
    records,
};
use fastctx_storage::Priority;
use fastctx_types::{focus::Focus, layout::ConversationDir};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, Read};

const FAMILY: &str = "focus";
const BYTES: usize = 4 * 1024 * 1024;

fn prepared(directory: &ConversationDir, value: &impl Serialize) -> io::Result<Prepared> {
    if serde_json::to_vec(value)?.len() > BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "focus record exceeds 4 MiB"));
    }
    Prepared::new(directory, value)
}

fn value<T: DeserializeOwned>(rows: &records::Rows<'_>, key: &str) -> fastctx_storage::Result<Option<T>> {
    rows.get::<Value>(FAMILY, key)?.map(|value| value.read(rows)).transpose()
}

/// The legacy files are read in place; a malformed one is set aside with its
/// reason and the other two still import.
pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    let source = directory.clone();
    records::ensure_step(directory, "focus-files", move || {
        let directory = &source;
        for (key, path) in [
            ("text", directory.focus()),
            ("opening", directory.focus_opening()),
            ("visible", directory.path().join("focus-visible.json")),
        ] {
            let file = match fastctx_platform::disk::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let mut bytes = Vec::new();
            file.take(BYTES as u64 + 1).read_to_end(&mut bytes)?;
            let original = match legacy(key, bytes) {
                Ok(original) => original,
                Err(error) if crate::record_migration::errors::damaged(&error) => {
                    crate::record_migration::set_aside(directory, "focus-files", &path, &error)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let kept = prepared(directory, &original)?;
            records::write_at(directory, crate::record_migration::scheduler::priority(), 4096, move |rows| {
                if !rows.exists(FAMILY, key)? {
                    rows.put(FAMILY, key, &kept.value)?;
                    kept.install(rows, FAMILY, key)?;
                }
                Ok(())
            })?;
            let imported: Option<serde_json::Value> = records::read(directory, |rows| value(rows, key))?;
            if imported.as_ref() != Some(&original) {
                return Err(io::Error::other("imported focus differs from its source; source preserved"));
            }
        }
        Ok(())
    })
}

fn legacy(key: &str, bytes: Vec<u8>) -> io::Result<serde_json::Value> {
    let invalid = |reason: &str| io::Error::new(io::ErrorKind::InvalidData, format!("{reason}; source preserved"));
    if bytes.len() > BYTES {
        return Err(invalid("legacy focus exceeds 4 MiB"));
    }
    let original = match key {
        "text" => {
            serde_json::Value::String(String::from_utf8(bytes).map_err(|_| invalid("legacy focus is not UTF-8"))?)
        }
        "opening" => serde_json::to_value(serde_json::from_slice::<Opening>(&bytes)?)?,
        "visible" => serde_json::to_value(serde_json::from_slice::<Focus>(&bytes)?)?,
        _ => unreachable!(),
    };
    if serde_json::to_vec(&original)?.len() > BYTES {
        return Err(invalid("legacy focus exceeds 4 MiB once encoded"));
    }
    Ok(original)
}

pub(super) fn opening(directory: &ConversationDir) -> io::Result<Option<Opening>> {
    prepare(directory)?;
    records::read(directory, |rows| value(rows, "opening"))
}

pub(super) fn text(directory: &ConversationDir) -> io::Result<String> {
    prepare(directory)?;
    records::read(directory, |rows| Ok(value(rows, "text")?.unwrap_or_default()))
}

pub(super) fn visible(directory: &ConversationDir) -> io::Result<Option<Focus>> {
    prepare(directory)?;
    records::read(directory, |rows| match value(rows, "visible")? {
        Some(focus) => Ok(Some(focus)),
        None => Ok(value::<Opening>(rows, "opening")?.map(|opening| opening.focus)),
    })
}

pub(super) fn begin(directory: &ConversationDir, focus: &Focus) -> io::Result<u64> {
    if serde_json::to_vec(focus)?.len() > BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "focus selection exceeds 4 MiB"));
    }
    prepare(directory)?;
    records::write_at(directory, Priority::Interactive, 64, |rows| {
        let next = rows
            .get::<u64>(FAMILY, "generation")?
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| fastctx_storage::Error::new("focus generation exhausted"))?;
        rows.put(FAMILY, "generation", &next)?;
        Ok(next)
    })
}

pub(super) fn keep(
    directory: &ConversationDir,
    generation: u64,
    note: Option<&str>,
    visible: &Focus,
    opening: Option<&Opening>,
) -> io::Result<bool> {
    let note = note.map(|text| prepared(directory, &text)).transpose()?;
    let visible = prepared(directory, visible)?;
    let opening = opening.map(|opening| prepared(directory, opening)).transpose()?;
    records::write_at(directory, Priority::Interactive, 12 * 1024, move |rows| {
        if rows.get::<u64>(FAMILY, "generation")? != Some(generation) {
            return Ok(false);
        }
        match note {
            Some(note) => {
                rows.put(FAMILY, "text", &note.value)?;
                note.install(rows, FAMILY, "text")?;
            }
            None => {
                rows.delete(FAMILY, "text")?;
                crate::record_payloads::link(rows.connection(), rows.conversation(), FAMILY, "text", None)?;
            }
        }
        rows.put(FAMILY, "visible", &visible.value)?;
        visible.install(rows, FAMILY, "visible")?;
        if let Some(opening) = opening {
            rows.put(FAMILY, "opening", &opening.value)?;
            opening.install(rows, FAMILY, "opening")?;
        }
        Ok(true)
    })
}
