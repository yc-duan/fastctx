//! Whole-conversation storage lifecycle. The writer's guard fences writes; a
//! per-conversation gate also fences file projections before any directory
//! can be recreated.
mod bundle;
mod deletion;
#[cfg(feature = "storage-drill")]
pub(crate) mod drill;
pub(crate) mod fences;
#[cfg(feature = "storage-drill")]
mod move_drill;
pub(crate) mod schema;
pub(crate) mod transfer;
#[cfg(feature = "storage-drill")]
pub(crate) mod transfer_drill;
pub(crate) use deletion::{begin, begin_with_account, finished, purge, resume};
use fastctx_storage::{Result, rusqlite::Connection};
use fastctx_types::layout::ConversationDir;
use std::{io, sync::LazyLock};

static FILES: LazyLock<crate::record_gates::Gates> =
    LazyLock::new(|| crate::record_gates::Gates::new(fastctx_platform::doing::Rank::RECORDS, "conversation files"));

pub(crate) fn check(db: &Connection, conversation: &str) -> Result<()> {
    let deleted: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM node_conversation_deletions WHERE name=?1)",
        [conversation],
        |row| row.get(0),
    )?;
    if deleted {
        return Err(fastctx_storage::Error::with_kind(
            fastctx_storage::ErrorKind::Invalid,
            "the conversation was deleted",
        ));
    }
    Ok(())
}

/// Runs file publication only while deletion cannot install its fence.
/// Callers must be on a blocking thread and must not reenter this gate.
pub(crate) fn with_live_files<T>(directory: &ConversationDir, work: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let (database, conversation) = crate::record_database::get(directory)?;
    let gate = FILES.get(directory.path());
    let _files = gate.lock();
    database.read_now(|db| check(db, &conversation)).map_err(io::Error::other)?;
    work()
}

/// Serializes lifecycle file changes, including quarantining an offer to a
/// deleted conversation. The caller must check its durable intent inside it.
pub(crate) fn with_file_gate<T>(directory: &ConversationDir, work: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let gate = FILES.get(directory.path());
    let _files = gate.lock();
    work()
}

pub(crate) fn deleted(db: &Connection, conversation: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM node_conversation_deletions WHERE name=?1)",
        [conversation],
        |row| row.get(0),
    )?)
}
