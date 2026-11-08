//! Path-based record entry points share the node's writer without owning it.

mod upgrade;

use fastctx_storage::{Database, WeakDatabase};
use fastctx_types::layout::{ConversationDir, WorldDir};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

struct Open {
    database: WeakDatabase,
    layout: std::sync::Arc<crate::gather::records::Layout>,
}
static OPEN: LazyLock<Mutex<HashMap<PathBuf, Open>>> = LazyLock::new(Default::default);

pub(crate) fn register(world: &WorldDir, database: &Database) -> io::Result<crate::record_migration::Service> {
    let layout = std::sync::Arc::new(crate::gather::records::Layout::load(world)?);
    let mut open = OPEN.lock().expect("record database registry");
    open.retain(|_, entry| entry.database.upgrade().is_some());
    open.insert(world.path().to_path_buf(), Open { database: database.downgrade(), layout: layout.clone() });
    drop(open);
    let service = crate::record_migration::Service::register(world.path(), database);
    if database.fault().is_none() {
        let excluded = database.read_background_now(|db| layout.hydration_exclusions(db)).map_err(io::Error::other)?;
        crate::record_migration::scheduler::hydrate(world.path(), database, &excluded, None)?;
    }
    Ok(service)
}

pub(crate) fn get(directory: &ConversationDir) -> io::Result<(Database, String)> {
    let (database, key) = registered(directory)?;
    let world = directory
        .path()
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or_else(|| io::Error::other("conversation has no world"))?;
    crate::conversation_store::transfer::ensure(world, &database, &key)?;
    let layout = OPEN
        .lock()
        .expect("record database registry")
        .get(world)
        .map(|entry| entry.layout.clone())
        .expect("registered layout");
    layout.ensure(world, &database, &key)?;
    Ok((database, key))
}

/// Lifecycle owners may cancel an import without waiting for it to finish.
pub(crate) fn registered(directory: &ConversationDir) -> io::Result<(Database, String)> {
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
        .and_then(|entry| entry.database.upgrade())
        .ok_or_else(|| io::Error::other("the conversation's database is not open"))?;
    Ok((database, key))
}

/// Resolve a node-owned metadata item's logical path, never an arbitrary file.
/// World-level queues use a separate scope from every conversation and guest.
pub(crate) fn item(path: &std::path::Path) -> io::Result<(ConversationDir, String, String, std::path::PathBuf)> {
    if !path.is_absolute() || path.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a record item needs an absolute normalized path"));
    }
    let (root, layout) = {
        let open = OPEN.lock().expect("record database registry");
        open.iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(root, entry)| (root.clone(), entry.layout.clone()))
    }
    .ok_or_else(|| io::Error::other("the item's database is not open"))?;
    let logical = layout.logical(path);
    let relative = logical.strip_prefix(&root).expect("selected database root");
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
    let folder = if logical != path && layout.original(&logical) != logical {
        logical.parent().expect("item parent")
    } else {
        path.parent().expect("item parent")
    };
    Ok((directory, family, key, folder.to_path_buf()))
}

/// Queue envelopes retain their original path-bound delivery identity.
pub(crate) fn original(path: &std::path::Path) -> PathBuf {
    let open = OPEN.lock().expect("record database registry");
    open.iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map_or_else(|| path.to_path_buf(), |(_, entry)| entry.layout.original(path))
}

pub(crate) fn reject_origin(world: &std::path::Path, owner: &str, path: &std::path::Path) -> io::Result<()> {
    let layout = OPEN.lock().expect("record database registry").get(world).map(|entry| entry.layout.clone());
    if let Some(layout) = layout {
        layout.reject_origin(world, owner, path)?;
    }
    Ok(())
}
pub(crate) fn rejected_origin(path: &std::path::Path) -> bool {
    OPEN.lock().expect("record database registry").values().any(|entry| entry.layout.rejected_origin(path))
}

/// The release before the World's format marker opens this database on every
/// start and creates this index there; a table in its place makes that release
/// stop where it opens, instead of serving a World whose files have moved here.
const OLDER_RELEASES: &str = "
DROP INDEX IF EXISTS book_deeds_began;
CREATE TABLE IF NOT EXISTS book_deeds_began(why TEXT NOT NULL);
INSERT INTO book_deeds_began(why) SELECT 'Fastctx 1.0.1-beta.13 and earlier kept these records in files; this World keeps them here'
WHERE NOT EXISTS(SELECT 1 FROM book_deeds_began);
";

/// Opens the node schema on a blocking thread before registering its writer.
pub(crate) fn open(world: &WorldDir) -> Database {
    let path = world.path().join("node.sqlite3");
    let guard = crate::asset_preservation::guard;
    fastctx_storage::Database::open_guarded_or_unavailable(&path, crate::composer::SCHEMA, guard, |db| {
        upgrade::payloads(db)?;
        upgrade::records(db)?;
        fastctx_storage::versions::apply(
            db,
            "node.attachment-pins",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::attachments::index::SCHEMA }],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.record-maps",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::records::maps::SCHEMA }],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.handovers",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::delivery::handovers::SCHEMA }],
        )?;
        fastctx_storage::versions::apply(
            db,
            "node.terminals",
            &[fastctx_storage::versions::Migration { version: 1, sql: crate::delivery::terminals::SCHEMA }],
        )?;
        upgrade::runs(db)?;
        crate::session_record::upgrade::apply(db)?;
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
        fastctx_storage::versions::apply(
            db,
            "node.older-releases",
            &[fastctx_storage::versions::Migration { version: 1, sql: OLDER_RELEASES }],
        )?;
        fastctx_sync::migration::initialize(db)?;
        crate::conversation_store::schema::initialize(db)?;
        crate::conversation_store::transfer::prepare(db, world)
    })
}
