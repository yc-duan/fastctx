//! Letting a conversation go from this machine once another took it whole.
//! With the node stopped, its records here must still read as the export the
//! other machine took, which that machine names by the export's fingerprint
//! (`super::status`). They go without a tombstone, so that it may come back
//! the way it left; its directory is set aside whole until the operator
//! removes it, and the machine keeps which export left and when.
use super::super::{bundle, schema};
use fastctx_platform::{disk, owned_directory};
use fastctx_storage::{
    Error, Result,
    rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior},
};
use fastctx_types::layout::WorldDir;
use std::{
    io,
    path::{Path, PathBuf},
};

/// Lets `conversation` go, the export taken of it named `fingerprint`; says
/// where its directory was set aside.
pub(crate) fn release(root: &Path, conversation: &str, fingerprint: &str) -> io::Result<PathBuf> {
    let root = std::path::absolute(root)?;
    let world = WorldDir::under(&root);
    let _owner = fastctx_platform::FileLock::try_acquire(&world.node_lock())?
        .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "stop the node before letting a conversation go"))?;
    if !world.kept()?.iter().any(|kept| kept.name == conversation) {
        return Err(io::Error::new(io::ErrorKind::NotFound, "the conversation is not kept on this node"));
    }
    let directory = super::directory(world.path(), conversation).map_err(io::Error::other)?;
    let kept = let_go(&world, directory.path(), conversation, fingerprint).map_err(io::Error::other)?;
    // The session registrations of the machine lead into the directory.
    for exchange in crate::retention::entries(&directory.path().join("claude"))? {
        let Some(session) = exchange.file_name() else { continue };
        let registration = fastctx_hosts::claude::exchange::sessions(world.path()).join(session);
        match disk::symlink_metadata(&registration) {
            Ok(metadata) if owned_directory::is_link(&metadata) => owned_directory::delete(&registration)?,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    disk::create_dir_all(world.released())?;
    owned_directory::move_to(directory.path(), &kept)?;
    Ok(kept)
}

/// Lets the records go and keeps that they left, saying where the directory
/// is to be set aside.
fn let_go(world: &WorldDir, directory: &Path, conversation: &str, fingerprint: &str) -> Result<PathBuf> {
    let mut db = Connection::open(world.path().join("node.sqlite3"))?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.pragma_update(None, "synchronous", "FULL")?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    db.pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)?;
    // A node last run before moves were kept has not made their tables.
    let creating = db.transaction()?;
    super::initialize(&creating)?;
    creating.execute_batch(fastctx_jobs::SCHEMA)?;
    creating.commit()?;
    let tables = schema::tables(&db)?;
    if let Some(kept) = unfinished(&db, conversation, fingerprint, &tables)? {
        return Ok(kept);
    }
    let exported = directory.join(bundle::FILE);
    if !disk::exists(&exported)? {
        return Err(Error::new("its export is not beside it; export it, move that, and let it go once taken"));
    }
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    super::super::check(&transaction, conversation)?;
    let importing: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM node_conversation_transfers WHERE name=?1 AND rejected=0)",
        [conversation],
        |row| row.get(0),
    )?;
    if importing {
        return Err(Error::new("the conversation is still being imported"));
    }
    let source = Connection::open_with_flags(&exported, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.execute_batch("PRAGMA trusted_schema=OFF; BEGIN")?;
    let manifest = bundle::manifest(&source, conversation, &tables)?;
    if bundle::fingerprint(&manifest)? != fingerprint {
        return Err(Error::new("the export beside it is not the one taken; let it go by the fingerprint of this one"));
    }
    drop(source);
    for proof in &manifest.tables {
        bundle::verify(&transaction, conversation, proof).map_err(|_| {
            Error::new("its records changed since that export; export it again, move that, and let it go once taken")
        })?;
    }
    for table in tables.iter().filter(|table| !manifest.tables.iter().any(|proof| proof.name == table.name)) {
        let remains: bool = transaction.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {} WHERE conversation=?1)", schema::quote(&table.name)),
            [conversation],
            |row| row.get(0),
        )?;
        if remains {
            return Err(Error::new(
                "new source assets were retained after that export; export and transfer them before release",
            ));
        }
    }
    let at = jiff::Timestamp::now().as_millisecond();
    let kept = world.released().join(format!("{conversation}-{at}"));
    {
        // Every row goes, so nothing derived from one is to be written.
        let _triggers = super::triggers::SuspendedTriggers::new(&transaction)?;
        for table in schema::children_first(&transaction, tables)? {
            transaction.execute(
                &format!("DELETE FROM {} WHERE conversation=?1", schema::quote(&table.name)),
                [conversation],
            )?;
        }
    }
    fastctx_jobs::forget(&transaction, conversation)?;
    transaction.execute("DELETE FROM node_conversation_arrivals WHERE name=?1", [conversation])?;
    transaction.execute(
        "INSERT INTO node_conversation_releases(name,fingerprint,at,kept) VALUES(?1,?2,?3,?4)",
        (conversation, fingerprint, at, kept.to_string_lossy()),
    )?;
    transaction.commit()?;
    Ok(kept)
}

/// Where the directory of a release stopped after its records went is to
/// be set aside: the last thing the conversation did here was leave by this
/// very export, and nothing of it is left.
fn unfinished(
    db: &Connection,
    conversation: &str,
    fingerprint: &str,
    tables: &[schema::Table],
) -> Result<Option<PathBuf>> {
    let arrived: bool =
        db.query_row("SELECT EXISTS(SELECT 1 FROM node_conversation_arrivals WHERE name=?1)", [conversation], |row| {
            row.get(0)
        })?;
    let left: Option<(String, String)> = db
        .query_row(
            "SELECT fingerprint,kept FROM node_conversation_releases WHERE name=?1 ORDER BY at DESC LIMIT 1",
            [conversation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((_, kept)) = left.filter(|(left, _)| !arrived && left == fingerprint) else { return Ok(None) };
    for table in tables {
        let remains: bool = db.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {} WHERE conversation=?1)", schema::quote(&table.name)),
            [conversation],
            |row| row.get(0),
        )?;
        if remains {
            return Ok(None);
        }
    }
    Ok(Some(PathBuf::from(kept)))
}
