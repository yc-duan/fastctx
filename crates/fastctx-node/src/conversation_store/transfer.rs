//! A pending bundle fences its conversation until every table has been verified.
//! Import work shares the legacy migrator's bounded workers and demand deadline.
use super::{bundle, schema};
use crate::record_migration::scheduler;
use fastctx_platform::disk;
use fastctx_storage::{Database, Error, Result, rusqlite::Connection};
use fastctx_types::layout::{ConversationDir, WorldDir};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::Path,
    sync::{LazyLock, Mutex},
};

mod cursors;
pub(crate) mod exporting;
mod importing;
mod quarantine;
mod rejection;
pub(super) use rejection::discard_journal;
pub(crate) mod release;
pub(crate) mod status;
mod triggers;
static REJECTED: LazyLock<Mutex<HashMap<std::path::PathBuf, HashMap<String, String>>>> =
    LazyLock::new(Default::default);
static PENDING: LazyLock<Mutex<HashMap<std::path::PathBuf, HashSet<String>>>> = LazyLock::new(Default::default);

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS node_conversation_transfers (
    name TEXT PRIMARY KEY, fingerprint TEXT, finished INTEGER NOT NULL DEFAULT 0,
    failure TEXT, rejected INTEGER NOT NULL DEFAULT 0, archive TEXT
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS node_conversation_transfer_progress (
    name TEXT NOT NULL, table_name TEXT NOT NULL, cursor BLOB NOT NULL,
    PRIMARY KEY(name,table_name)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS node_conversation_import_rows (
    name TEXT NOT NULL, table_name TEXT NOT NULL, row_key BLOB NOT NULL,
    PRIMARY KEY(name,table_name,row_key)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS node_record_state (
    name TEXT PRIMARY KEY, value INTEGER NOT NULL
) WITHOUT ROWID;
";

/// What a move by hand leaves on each end (`release`): the export this machine
/// last took of a conversation, and each time one left it, by the fingerprint
/// of the export taken elsewhere and where its directory was set aside. A move
/// is a person's act: an arrival stays until the conversation leaves again,
/// and a departure is kept for good, to know the conversation if it returns.
const MOVES: &str = "
CREATE TABLE IF NOT EXISTS node_conversation_arrivals (
    name TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, source_machine TEXT NOT NULL, at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS node_conversation_releases (
    name TEXT NOT NULL, fingerprint TEXT NOT NULL, at INTEGER NOT NULL, kept TEXT NOT NULL,
    PRIMARY KEY (name, at)
) WITHOUT ROWID;
";

pub(super) fn initialize(db: &Connection) -> Result<()> {
    fastctx_storage::versions::apply(
        db,
        "node.conversation-imports",
        &[
            fastctx_storage::versions::Migration { version: 1, sql: SCHEMA },
            fastctx_storage::versions::Migration { version: 2, sql: MOVES },
        ],
    )?;
    for (name, definition) in [("failure", "TEXT"), ("rejected", "INTEGER NOT NULL DEFAULT 0"), ("archive", "TEXT")] {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo('node_conversation_transfers') WHERE name=?1)",
            [name],
            |row| row.get(0),
        )?;
        if !exists {
            db.execute_batch(&format!("ALTER TABLE node_conversation_transfers ADD COLUMN {name} {definition}"))?;
        }
    }
    Ok(())
}

/// Only discovers bundles and installs admission fences; it imports no data.
pub(crate) fn prepare(db: &Connection, world: &WorldDir) -> Result<()> {
    db.execute("DELETE FROM node_record_state WHERE name='legacy-discovery-complete'", [])?;
    for kept in world.kept().map_err(|error| Error::new(error.to_string()))? {
        // A room that cannot be looked into now is looked at on the next open;
        // it never makes the whole database unavailable.
        match disk::exists(kept.directory.path().join(bundle::FILE)) {
            Ok(true) => {
                db.execute("INSERT OR IGNORE INTO node_conversation_transfers(name) VALUES(?1)", [&kept.name])?;
            }
            Ok(false) => {}
            Err(_) => fastctx_platform::readings::counter("migration.unreadable_rooms").add(1),
        }
    }
    let entries = db
        .prepare("SELECT name,rejected,failure FROM node_conversation_transfers")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?, row.get::<_, Option<String>>(2)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut pending = HashSet::new();
    let mut rejected = HashMap::new();
    for (name, failed, reason) in entries {
        if failed {
            if disk::exists(path(world.path(), &name)?).unwrap_or(false) {
                db.execute("UPDATE node_conversation_transfers SET fingerprint=NULL,finished=0,failure=NULL,rejected=0,archive=NULL WHERE name=?1", [&name])?;
                db.execute("DELETE FROM node_conversation_transfer_progress WHERE name=?1", [&name])?;
            } else {
                rejected.insert(name, reason.unwrap_or_else(|| "the conversation import was rejected".into()));
                continue;
            }
        }
        pending.insert(name);
    }
    for conversation in &pending {
        scheduler::invalidate(world.path(), conversation);
    }
    PENDING.lock().expect("conversation transfers").insert(world.path().to_path_buf(), pending);
    fastctx_platform::readings::gauge("migration.rejected_imports").set(rejected.len() as i64);
    REJECTED.lock().expect("rejected imports").insert(world.path().to_path_buf(), rejected);
    Ok(())
}

pub(crate) fn pending(world: &Path, conversation: &str) -> bool {
    PENDING.lock().expect("conversation transfers").get(world).is_some_and(|names| names.contains(conversation))
}

pub(crate) fn ensure(world: &Path, database: &Database, conversation: &str) -> io::Result<()> {
    if !pending(world, conversation) {
        return Ok(());
    }
    let (root, scope, database) = (world.to_path_buf(), conversation.to_string(), database.clone());
    scheduler::request(world, conversation, "conversation-bundle", move || importing::run(&root, &database, &scope))
}

pub(crate) async fn wait(
    node: &crate::Node,
    id: &fastctx_types::ResidentId,
) -> std::result::Result<(), fastctx_types::ipc::Failure> {
    let (world, conversation) = (node.world.path().to_path_buf(), id.to_string());
    // Asked again as the migration progresses, at least each second, with
    // no thread held meanwhile; ten seconds at most, as a request waits.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if !pending(&world, &conversation) {
            return Ok(());
        }
        let progressed = node.record_migration.wait_progress();
        let (root, scope, database) = (world.clone(), conversation.clone(), node.database.clone());
        let moved = scheduler::ask(&world, &conversation, "conversation-bundle", move || {
            importing::run(&root, &database, &scope)
        })
        .map_err(crate::record_migration::failure)?;
        if moved {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(crate::record_migration::failure(io::Error::new(
                io::ErrorKind::WouldBlock,
                "record migration is busy: the ten-second wait expired; retry with backoff",
            )));
        }
        let _ = tokio::time::timeout_at(
            deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(1)),
            progressed,
        )
        .await;
    }
}

pub(crate) fn scan(node: &crate::Node) -> io::Result<bool> {
    let names: Vec<_> =
        PENDING.lock().expect("conversation transfers").get(node.world.path()).into_iter().flatten().cloned().collect();
    let mut done = true;
    for name in names {
        match ensure(node.world.path(), &node.database, &name) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => done = false,
            Err(error) => {
                done = false;
                node.log.line(format_args!("importing conversation {name}: {error}"));
            }
        }
    }
    Ok(done)
}

fn path(world: &Path, conversation: &str) -> Result<std::path::PathBuf> {
    if conversation.is_empty() || conversation.starts_with('.') || conversation.contains(['/', '\\']) {
        return Err(bundle::invalid("invalid conversation bundle name"));
    }
    Ok(world.join("conversations").join(conversation).join(bundle::FILE))
}

pub(super) fn install_fences(db: &Connection, table: &schema::Table) -> Result<()> {
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        let scope = if operation == "DELETE" { "OLD.conversation" } else { "NEW.conversation" };
        let old = if operation == "UPDATE" { " OR name=OLD.conversation" } else { "" };
        db.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS {trigger}; CREATE TRIGGER {trigger} BEFORE {operation} ON {table}
            WHEN EXISTS(SELECT 1 FROM node_conversation_transfers WHERE rejected=0 AND (name={scope}{old}))
            BEGIN SELECT RAISE(ABORT,'the conversation is being imported'); END;",
            trigger = schema::quote(&format!("{}_pending_import_{}", table.name, operation.to_lowercase())),
            table = schema::quote(&table.name)
        ))?;
    }
    Ok(())
}

pub(super) fn directory(world: &Path, conversation: &str) -> Result<ConversationDir> {
    Ok(ConversationDir::at(path(world, conversation)?.parent().expect("bundle parent")))
}

pub(super) fn cancelled(world: &Path, conversation: &str) {
    if let Some(pending) = PENDING.lock().expect("conversation transfers").get_mut(world) {
        pending.remove(conversation);
    }
    if let Some(rejected) = REJECTED.lock().expect("rejected imports").get_mut(world) {
        rejected.remove(conversation);
        fastctx_platform::readings::gauge("migration.rejected_imports").set(rejected.len() as i64);
    }
}

pub(crate) fn rejection(world: &Path, conversation: &str) -> Option<String> {
    REJECTED.lock().expect("rejected imports").get(world)?.get(conversation).cloned()
}

fn record_rejection(world: &Path, conversation: &str, reason: String) {
    if let Some(pending) = PENDING.lock().expect("conversation transfers").get_mut(world) {
        pending.remove(conversation);
    }
    let mut all = REJECTED.lock().expect("rejected imports");
    let rejected = all.entry(world.to_path_buf()).or_default();
    rejected.insert(conversation.to_string(), reason);
    fastctx_platform::readings::gauge("migration.rejected_imports").set(rejected.len() as i64);
}
