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

pub(crate) mod exporting;
mod importing;
mod triggers;
static PENDING: LazyLock<Mutex<HashMap<std::path::PathBuf, HashSet<String>>>> = LazyLock::new(Default::default);

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS node_conversation_transfers (
    name TEXT PRIMARY KEY, fingerprint TEXT, finished INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS node_conversation_transfer_progress (
    name TEXT NOT NULL, table_name TEXT NOT NULL, cursor BLOB NOT NULL,
    PRIMARY KEY(name,table_name)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS node_record_state (
    name TEXT PRIMARY KEY, value INTEGER NOT NULL
) WITHOUT ROWID;
";

/// Only discovers bundles and installs admission fences; it imports no data.
pub(crate) fn prepare(db: &Connection, world: &WorldDir) -> Result<()> {
    db.execute("DELETE FROM node_record_state WHERE name='legacy-discovery-complete'", [])?;
    for kept in world.kept().map_err(|error| Error::new(error.to_string()))? {
        if disk::exists(kept.directory.path().join(bundle::FILE)).map_err(|error| Error::new(error.to_string()))? {
            db.execute("INSERT OR IGNORE INTO node_conversation_transfers(name) VALUES(?1)", [&kept.name])?;
        }
    }
    let entries = db
        .prepare("SELECT name,finished FROM node_conversation_transfers")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut pending = HashSet::new();
    for (name, finished) in entries {
        let path = path(world.path(), &name)?;
        if finished && !disk::exists(path).map_err(|error| Error::new(error.to_string()))? {
            db.execute("DELETE FROM node_conversation_transfer_progress WHERE name=?1", [&name])?;
            db.execute("DELETE FROM node_conversation_transfers WHERE name=?1", [&name])?;
        } else {
            pending.insert(name);
        }
    }
    PENDING.lock().expect("conversation transfers").insert(world.path().to_path_buf(), pending);
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
    let (world, database, conversation) = (node.world.path().to_path_buf(), node.database.clone(), id.to_string());
    if !pending(&world, &conversation) {
        return Ok(());
    }
    fastctx_platform::task::spawn_blocking(move || ensure(&world, &database, &conversation))
        .await
        .map_err(|error| fastctx_types::ipc::Failure::new(error.to_string()))?
        .map_err(crate::record_migration::failure)
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
        return Err(Error::new("invalid conversation bundle name"));
    }
    Ok(world.join("conversations").join(conversation).join(bundle::FILE))
}

pub(super) fn install_fences(db: &Connection, table: &schema::Table) -> Result<()> {
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        let scope = if operation == "DELETE" { "OLD.conversation" } else { "NEW.conversation" };
        let old = if operation == "UPDATE" { " OR name=OLD.conversation" } else { "" };
        db.execute_batch(&format!(
            "CREATE TRIGGER IF NOT EXISTS {} BEFORE {operation} ON {}
            WHEN EXISTS(SELECT 1 FROM node_conversation_transfers WHERE name={scope}{old})
            BEGIN SELECT RAISE(ABORT,'the conversation is being imported'); END;",
            schema::quote(&format!("{}_pending_import_{}", table.name, operation.to_lowercase())),
            schema::quote(&table.name)
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
}
