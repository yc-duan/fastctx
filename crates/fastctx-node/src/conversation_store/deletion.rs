//! Durable deletion intent and bounded, restartable garbage collection.
use super::{FILES, schema};
use fastctx_platform::readings;
use fastctx_storage::{Database, Priority, Result};
use fastctx_types::layout::ConversationDir;
use std::{io, sync::LazyLock};

const BATCH: usize = 128;
static ROWS: LazyLock<&'static readings::Counter> = LazyLock::new(|| readings::counter("records.deleted_rows"));
static BATCH_TIME: LazyLock<&'static readings::Timing> = LazyLock::new(|| readings::timing("records.deletion_batch"));

/// The tombstone and pending cleanup are the same row and commit together.
pub(crate) fn begin(directory: &ConversationDir) -> io::Result<()> {
    begin_with_account(directory, None, None)
}

pub(crate) fn begin_with_account(
    directory: &ConversationDir,
    account: Option<Vec<u8>>,
    fork_source: Option<Vec<u8>>,
) -> io::Result<()> {
    let gate = FILES.get(directory.path());
    let _files = gate.lock();
    let (database, conversation) = crate::record_database::registered(directory)?;
    let completed = conversation.clone();
    database
        .write_now_at(Priority::Control, conversation.len(), move |db| {
            db.execute(
                "INSERT OR IGNORE INTO node_conversation_deletions(name,started,account,fork_source) VALUES(?1,?2,?3,?4)",
                (&conversation, jiff::Timestamp::now().as_millisecond(), account, fork_source),
            )?;
            db.execute("DELETE FROM node_conversation_transfer_progress WHERE name=?1", [&conversation])?;
            db.execute("DELETE FROM node_conversation_transfers WHERE name=?1", [&conversation])?;
            db.execute("DELETE FROM migration_born WHERE name=?1", [&conversation])?;
            fastctx_jobs::forget(db, &conversation)?;
            // What the content database keeps of it goes too, as a job of
            // its own unit: the line above drops the conversation's.
            crate::content_upkeep::forget_in(db, &conversation)?;
            Ok(())
        })
        .map_err(io::Error::other)?;
    let world = crate::record_migration::world(directory)?;
    crate::record_migration::scheduler::invalidate(&world, &completed);
    super::transfer::cancelled(&world, &completed);
    Ok(())
}

pub(crate) fn purge(database: &Database, conversation: &str) -> Result<u64> {
    let tables = database.read_background_now(|db| {
        let purged: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM node_conversation_deletions WHERE name=?1 AND purged=1)",
            [conversation],
            |row| row.get(0),
        )?;
        // The tombstone, written when the deletion began, fences every
        // conversation row, so once a purge has finished none is left to
        // remove. Transfer bookkeeping below can still arrive and must be cleared.
        if purged { Ok(Vec::new()) } else { schema::tables(db) }
    })?;
    // Children precede parents, so cascading deletes never turn a small batch
    // into an unbounded one, and every deleted value contributes to the count.
    let tables = ordered(database, tables)?;
    for table in tables {
        let quoted = schema::quote(&table.name);
        let key = if table.primary.is_empty() {
            "rowid".to_string()
        } else {
            table.primary.iter().map(|column| schema::quote(column)).collect::<Vec<_>>().join(",")
        };
        let size = table
            .writable
            .iter()
            .map(|column| format!("coalesce(length(CAST({} AS BLOB)),0)", schema::quote(column)))
            .collect::<Vec<_>>()
            .join("+");
        loop {
            let conversation = conversation.to_string();
            let (quoted, key, size) = (quoted.clone(), key.clone(), size.clone());
            let began = std::time::Instant::now();
            // Relaxed: the tombstone is on the disk, a batch lost to a power
            // cut is deleted again, and the purge ends with a durable commit.
            let count = database.write_relaxed_now_at(Priority::Background, 1024, move |db| {
                let pending: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM node_conversation_deletions WHERE name=?1)",
                    [&conversation],
                    |row| row.get(0),
                )?;
                if !pending {
                    return Err(fastctx_storage::Error::new("deleting records requires a tombstone"));
                }
                let mut statement = db.prepare_cached(&format!(
                    "SELECT {key},{size} FROM {quoted} WHERE conversation=?1 LIMIT {BATCH}"
                ))?;
                let keys = statement.column_count() - 1;
                let mut cursor = statement.query([&conversation])?;
                let mut values = Vec::new();
                let mut bytes = 0i64;
                let mut count = 0;
                while let Some(row) = cursor.next()? {
                    for index in 0..keys {
                        values.push(row.get::<_, fastctx_storage::rusqlite::types::Value>(index)?);
                    }
                    bytes += row.get::<_, i64>(keys)?;
                    count += 1;
                }
                drop(cursor);
                drop(statement);
                if count > 0 {
                    let tuple = format!("({})", vec!["?"; keys].join(","));
                    let selected = vec![tuple; count].join(",");
                    db.execute(
                        &format!("DELETE FROM {quoted} WHERE ({key}) IN (VALUES {selected})"),
                        fastctx_storage::rusqlite::params_from_iter(values),
                    )?;
                    db.execute(
                        "UPDATE node_conversation_deletions SET bytes=bytes+?2 WHERE name=?1",
                        (&conversation, bytes),
                    )?;
                }
                Ok(count)
            })?;
            BATCH_TIME.record(began.elapsed());
            ROWS.add(count as u64);
            if count == 0 {
                break;
            }
        }
    }
    super::transfer::discard_journal(database, conversation)?;
    let conversation = conversation.to_string();
    database.write_now_at(Priority::Background, conversation.len(), move |db| {
        db.execute("UPDATE node_conversation_deletions SET purged=1 WHERE name=?1", [&conversation])?;
        // An offer rejected while the deletion went on has nothing left to wait for.
        db.execute("DELETE FROM node_conversation_transfer_progress WHERE name=?1", [&conversation])?;
        db.execute("DELETE FROM node_conversation_transfers WHERE name=?1", [&conversation])?;
        fastctx_jobs::forget(db, &conversation)?;
        Ok(db.query_row("SELECT bytes FROM node_conversation_deletions WHERE name=?1", [&conversation], |row| {
            row.get::<_, i64>(0)
        })? as u64)
    })
}

fn ordered(database: &Database, tables: Vec<schema::Table>) -> Result<Vec<schema::Table>> {
    database.read_background_now(|db| schema::children_first(db, tables))
}

/// Resumes SQL cleanup even when the directory has already disappeared.
pub(crate) fn resume(database: &Database) -> io::Result<()> {
    let mut after = String::new();
    loop {
        let names = database.read_background_now(|db| {
            Ok(db.prepare("SELECT name FROM node_conversation_deletions WHERE name>?1 AND purged=0 ORDER BY name LIMIT 16")?
                .query_map([&after], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        }).map_err(io::Error::other)?;
        let Some(last) = names.last() else { return Ok(()) };
        after = last.clone();
        for name in names {
            purge(database, &name).map_err(io::Error::other)?;
        }
    }
}

pub(crate) fn finished(database: &Database, name: String) -> io::Result<()> {
    database
        .write_now_at(Priority::Background, name.len(), move |db| {
            db.execute(
                "UPDATE node_conversation_deletions SET finished=1,account=NULL,fork_source=NULL WHERE name=?1 AND purged=1",
                [name],
            )?;
            Ok(())
        })
        .map_err(io::Error::other)
}
