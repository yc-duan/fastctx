//! One current history receipt per conversation, at most 16,384 topic hashes and
//! 4 MiB encoded. Payload pieces share unchanged hash blocks. Replaced roots are
//! reclaimed by payload collection; the current receipt lasts until deletion.
use super::Cursor;
use crate::{
    record_value::{Prepared, Value},
    records,
};
use fastctx_storage::Priority;
use fastctx_types::layout::ConversationDir;
use std::io::{self, Read};

const FAMILY: &str = "history-heard";
const BYTES: usize = 4 * 1024 * 1024;
const TOPICS: usize = 16_384;

pub(super) fn decode(bytes: &[u8]) -> io::Result<Cursor> {
    if bytes.len() > BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "history receipt exceeds 4 MiB"));
    }
    let cursor: Cursor = serde_json::from_slice(bytes)?;
    validate(&cursor)?;
    Ok(cursor)
}

fn validate(cursor: &Cursor) -> io::Result<()> {
    let digest = |text: &str| text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit());
    if cursor.topics.len() > TOPICS
        || !digest(&cursor.scope)
        || cursor.topics.iter().any(|(key, value)| !digest(key) || !digest(value))
        || cursor.versions.keys().any(|key| !cursor.topics.contains_key(key))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "history receipt has invalid hashes or more than 16384 topics",
        ));
    }
    if serde_json::to_vec(cursor)?.len() > BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "history receipt exceeds 4 MiB"));
    }
    Ok(())
}

fn current(directory: &ConversationDir) -> io::Result<Option<Cursor>> {
    records::read_background(directory, |rows| {
        rows.get::<Value>(FAMILY, "current")?.map(|value| value.read(rows)).transpose()
    })
}

/// The legacy receipt is read in place; a malformed one is set aside with its
/// reason, as if it had never been written.
pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    let source = directory.clone();
    records::ensure_step(directory, "history-heard-file", move || {
        let directory = &source;
        let path = directory.books().join("history-heard.json");
        let file = match fastctx_platform::disk::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(BYTES as u64 + 1).read_to_end(&mut bytes)?;
        let cursor = match decode(&bytes) {
            Ok(cursor) => cursor,
            Err(error) if crate::record_migration::errors::damaged(&error) => {
                return crate::record_migration::set_aside(directory, "history-heard-file", &path, &error);
            }
            Err(error) => return Err(error),
        };
        let prepared = Prepared::new(directory, &cursor)?;
        records::write_at(directory, crate::record_migration::scheduler::priority(), 4096, move |rows| {
            if !rows.exists(FAMILY, "current")? {
                rows.put(FAMILY, "current", &prepared.value)?;
                prepared.install(rows, FAMILY, "current")?;
            }
            Ok(())
        })?;
        if current(directory)?.as_ref() != Some(&cursor) {
            return Err(io::Error::other("imported history receipt differs from its source; source preserved"));
        }
        Ok(())
    })
}

pub(super) fn read(directory: &ConversationDir) -> io::Result<Option<Cursor>> {
    prepare(directory)?;
    current(directory)
}

pub(super) fn keep(directory: &ConversationDir, cursor: &Cursor, revision: u64) -> io::Result<()> {
    validate(cursor)?;
    prepare(directory)?;
    let prepared = Prepared::new(directory, cursor)?;
    records::write_at(directory, Priority::Interactive, 4096, move |rows| {
        // A newer acknowledgement or compaction may have arrived during staging.
        if crate::context::seen_revision(rows, "history")? != Some(revision) {
            return Ok(());
        }
        rows.put(FAMILY, "current", &prepared.value)?;
        prepared.install(rows, FAMILY, "current")
    })
}
