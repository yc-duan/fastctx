//! Table batches and their cursors commit together. Admission remains fenced
//! across crashes, including a crash after verification but before file removal.
use super::{
    bundle,
    cursors::{decode, encode},
    path, scheduler, schema,
};
use fastctx_platform::disk;
use fastctx_storage::{
    Database, Error, ErrorKind, Priority, Result,
    rusqlite::{Connection, OpenFlags, OptionalExtension},
};
use std::{io, path::Path};

pub(super) fn run(world: &Path, database: &Database, conversation: &str) -> io::Result<()> {
    let failure = database
        .read_background_now(|db| {
            Ok(db
                .query_row("SELECT failure FROM node_conversation_transfers WHERE name=?1", [conversation], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .optional()?
                .flatten())
        })
        .map_err(io::Error::other)?;
    if let Some(reason) = failure {
        return super::rejection::resume(world, database, conversation, reason);
    }
    match import(world, database, conversation) {
        Err(error) if error.kind() == ErrorKind::Invalid => {
            super::rejection::resume(world, database, conversation, error.to_string())
        }
        Err(error) => {
            let kind = if error.kind() == ErrorKind::Busy { io::ErrorKind::WouldBlock } else { io::ErrorKind::Other };
            Err(io::Error::new(kind, error))
        }
        Ok(()) => Ok(()),
    }
}

// Only errors reading the offered database that say what it holds is bad
// are attributed to its contents. Target database damage, filesystem
// admission failures and errors nobody classified remain retryable: a good
// offer is never set aside for them.
fn source_error(error: impl Into<Error>) -> Error {
    let error = error.into();
    match error.kind() {
        ErrorKind::Invalid | ErrorKind::Corrupt => bundle::invalid(error.to_string()),
        _ => error,
    }
}

/// The offered database, read-only in one snapshot, its manifest and the
/// manifest's digest, which names the offer.
fn offer(path: &Path, conversation: &str, tables: &[schema::Table]) -> Result<(Connection, bundle::Manifest, String)> {
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(source_error)?;
    source.execute_batch("PRAGMA trusted_schema=OFF; BEGIN").map_err(source_error)?;
    let manifest = bundle::manifest(&source, conversation, tables).map_err(source_error)?;
    let digest = bundle::fingerprint(&manifest)?;
    Ok((source, manifest, digest))
}

fn import(world: &Path, database: &Database, conversation: &str) -> Result<()> {
    let path = path(world, conversation)?;
    if !disk::exists(&path)? {
        let finished = database.read_background_now(|db| {
            super::super::check(db, conversation)?;
            Ok(db
                .query_row("SELECT finished FROM node_conversation_transfers WHERE name=?1", [conversation], |row| {
                    row.get::<_, bool>(0)
                })
                .optional()?)
        })?;
        if finished != Some(false) {
            return complete(world, database, conversation);
        }
        return Err(bundle::invalid("an unfinished conversation import is missing its bundle"));
    }
    let tables = database.read_background_now(schema::tables)?;
    let state = database.read_background_now(|db| {
        Ok(db
            .query_row(
                "SELECT finished,fingerprint FROM node_conversation_transfers WHERE name=?1",
                [conversation],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?)
    })?;
    if let Some((true, recorded)) = state {
        // The rows are in and checked. A stop between marking that and removing
        // the bundle leaves the file: it goes if it is the bundle they came from,
        // and is set aside otherwise; the finished import is never rolled back.
        let same = recorded
            .is_some_and(|recorded| offer(&path, conversation, &tables).is_ok_and(|(_, _, found)| found == recorded));
        if same {
            match disk::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        } else {
            crate::record_migration::archive::leftover(
                world,
                conversation,
                &path,
                "a bundle found where a finished import's bundle was",
            )?;
        }
        return complete(world, database, conversation);
    }
    let original = crate::record_migration::archive::fingerprint(&path)?;
    let (source, manifest, signature) = offer(&path, conversation, &tables)?;
    let scope = conversation.to_string();
    database.write_now_at(scheduler::priority(), 256, move |db| {
        super::super::check(db, &scope)?;
        let prior: Option<String> =
            db.query_row("SELECT fingerprint FROM node_conversation_transfers WHERE name=?1", [&scope], |row| {
                row.get(0)
            })?;
        if prior.as_ref().is_some_and(|prior| prior != &signature) {
            return Err(bundle::invalid("the pending conversation bundle was replaced"));
        }
        db.execute("UPDATE node_conversation_transfers SET fingerprint=?2 WHERE name=?1", (&scope, signature))?;
        Ok(())
    })?;
    for proof in &manifest.tables {
        scheduler::checkpoint().map_err(Error::from)?;
        bundle::verify(&source, conversation, proof).map_err(source_error)?;
    }
    let offered =
        tables.into_iter().filter(|table| manifest.tables.iter().any(|proof| proof.name == table.name)).collect();
    let mut order = database.read_background_now(|db| schema::children_first(db, offered))?;
    order.reverse();
    for table in order {
        let mut after = database.read_background_now(|db| {
            let cursor: Option<Vec<u8>> = db
                .query_row(
                    "SELECT cursor FROM node_conversation_transfer_progress WHERE name=?1 AND table_name=?2",
                    (conversation, &table.name),
                    |row| row.get(0),
                )
                .optional()?;
            cursor.map(|bytes| decode(&bytes)).transpose().map(Option::unwrap_or_default)
        })?;
        loop {
            scheduler::checkpoint().map_err(Error::from)?;
            let batch = bundle::page(&source, conversation, &table, &after).map_err(source_error)?;
            let Some(last) = batch.last() else {
                break;
            };
            after = bundle::key(&table, last)?;
            let cursor = encode(&after)?;
            let bytes = batch.iter().flatten().map(bundle::value_bytes).sum::<usize>() * 2 + cursor.len();
            let (table, scope) = (table.clone(), conversation.to_string());
            database.write_now_at(scheduler::priority(),bytes,move |db| {
                super::super::check(db,&scope)?;
                let _triggers=super::triggers::SuspendedTriggers::new(db)?;
                for row in batch {
                    if bundle::put(db,&table,&row)? {
                        let key=encode(&bundle::key(&table,&row)?)?;
                        db.execute("INSERT INTO node_conversation_import_rows(name,table_name,row_key) VALUES(?1,?2,?3)",(&scope,&table.name,key))?;
                    }
                }
                db.execute("INSERT INTO node_conversation_transfer_progress(name,table_name,cursor) VALUES(?1,?2,?3) ON CONFLICT(name,table_name) DO UPDATE SET cursor=excluded.cursor",(&scope,&table.name,cursor))?;
                Ok(())
            })?;
        }
    }
    for proof in &manifest.tables {
        scheduler::checkpoint().map_err(Error::from)?;
        database.read_background_now(|db| bundle::verify(db, conversation, proof))?;
    }
    drop(source);
    let from = manifest.source_machine.clone();
    let directory = super::directory(world, conversation)?;
    super::super::with_file_gate(&directory, || {
        let current = match crate::record_migration::archive::fingerprint(&path) {
            Ok(current) => current,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the conversation bundle disappeared during import",
                ));
            }
            Err(error) => return Err(error),
        };
        if current != original {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the conversation bundle changed during import"));
        }
        let scope = conversation.to_string();
        database
            .write_now_at(Priority::Background, 256, move |db| {
                super::super::check(db, &scope)?;
                db.execute("UPDATE node_conversation_transfers SET finished=1 WHERE name=?1", [&scope])?;
                // The receipt the machine it came from lets it go by.
                db.execute(
                    "INSERT INTO node_conversation_arrivals(name,fingerprint,source_machine,at)
                     SELECT name,fingerprint,?2,?3 FROM node_conversation_transfers WHERE name=?1
                     ON CONFLICT(name) DO UPDATE SET fingerprint=excluded.fingerprint,
                         source_machine=excluded.source_machine, at=excluded.at",
                    (&scope, from, jiff::Timestamp::now().as_millisecond()),
                )?;
                Ok(())
            })
            .map_err(io::Error::other)?;
        disk::remove_file(&path)
    })?;
    complete(world, database, conversation)
}

fn complete(world: &Path, database: &Database, conversation: &str) -> Result<()> {
    super::discard_journal(database, conversation)?;
    let scope = conversation.to_string();
    let removed = database.write_now_at(Priority::Background, 128, move |db| {
        db.execute("DELETE FROM node_conversation_transfer_progress WHERE name=?1", [&scope])?;
        Ok(db.execute("DELETE FROM node_conversation_transfers WHERE name=?1", [&scope])? != 0)
    })?;
    scheduler::hydrate(world, database, &std::collections::HashSet::new(), Some(conversation)).map_err(Error::from)?;
    super::cancelled(world, conversation);
    if removed {
        fastctx_platform::readings::counter("storage.conversations_imported").add(1);
    }
    Ok(())
}
