//! Path-based record entry points share the node's writer without owning it.

use fastctx_storage::{Database, WeakDatabase};
use fastctx_types::layout::{ConversationDir, WorldDir};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

static OPEN: LazyLock<Mutex<HashMap<PathBuf, WeakDatabase>>> = LazyLock::new(Default::default);

pub(crate) fn register(world: &WorldDir, database: &Database) {
    let mut open = OPEN.lock().expect("record database registry");
    open.retain(|_, database| database.upgrade().is_some());
    open.insert(world.path().to_path_buf(), database.downgrade());
}

pub(crate) fn get(directory: &ConversationDir) -> io::Result<(Database, String)> {
    let conversations = directory.path().parent().ok_or_else(|| io::Error::other("conversation has no parent"))?;
    let world = conversations.parent().ok_or_else(|| io::Error::other("conversation has no world"))?;
    let key = directory
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("invalid conversation identity"))?
        .to_string();
    let database = OPEN
        .lock()
        .expect("record database registry")
        .get(world)
        .and_then(WeakDatabase::upgrade)
        .ok_or_else(|| io::Error::other("the conversation's database is not open"))?;
    Ok((database, key))
}

/// Resolve a node-owned metadata item's logical path, never an arbitrary file.
/// World-level queues use a separate scope from every conversation and guest.
pub(crate) fn item(path: &std::path::Path) -> io::Result<(ConversationDir, String, String, std::path::PathBuf)> {
    if !path.is_absolute() || path.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a record item needs an absolute normalized path"));
    }
    let root = {
        let open = OPEN.lock().expect("record database registry");
        open.keys().filter(|root| path.starts_with(root)).max_by_key(|root| root.components().count()).cloned()
    }
    .ok_or_else(|| io::Error::other("the item's database is not open"))?;
    let relative = path.strip_prefix(&root).expect("selected database root");
    let mut parts = relative.components();
    let first = parts.next();
    let (directory, tail) = if first.is_some_and(|part| part.as_os_str() == "conversations") {
        let scope = parts.next().ok_or_else(|| io::Error::other("record has no conversation"))?;
        (ConversationDir::at(&root.join("conversations").join(scope.as_os_str())), parts.as_path().to_path_buf())
    } else {
        (ConversationDir::at(&root.join("conversations/@node")), relative.to_path_buf())
    };
    let key = tail
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("invalid record item name"))?
        .to_string();
    let parent = tail.parent().ok_or_else(|| io::Error::other("record item has no family"))?;
    let family = format!("items/{}", parent.to_string_lossy().replace('\\', "/"));
    Ok((directory, family, key, path.parent().expect("item parent").to_path_buf()))
}

/// Opens the node schema on a blocking thread before registering its writer.
pub(crate) fn open(world: &WorldDir) -> Database {
    fastctx_storage::Database::open_or_unavailable(&world.path().join("node.sqlite3"), crate::composer::SCHEMA, |db| {
        fastctx_storage::versions::apply(
            db,
            "node.record-payloads",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::record_payloads::SCHEMA }],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.record-maps",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::records::maps::SCHEMA }],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.terminals",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::delivery::terminals::SCHEMA }],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.run-facts",
            &[
                fastctx_storage::versions::Migration { version: 1, sql: crate::run_facts::SCHEMA },
                fastctx_storage::versions::Migration { version: 2, sql: crate::run_facts::SCHEMA },
            ],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.session-events",
            &[
                fastctx_storage::versions::Migration { version: 1, sql: crate::session_record::SCHEMA },
                fastctx_storage::versions::Migration { version: 2, sql: crate::session_record::SCHEMA },
            ],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.records",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::records::SCHEMA }],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.input-membership",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::inbox::presence::SCHEMA }],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.goal-projection",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::goals::SCHEMA }],
        )?;
        crate::conversation_store::schema::initialize(db)
    })
}
