//! Table batches and their cursors commit together. Admission remains fenced
//! across crashes, including a crash after verification but before file removal.
use super::{PENDING, bundle, path, scheduler, schema};
use fastctx_platform::disk;
use fastctx_storage::{
    Database, Error, Priority, Result,
    rusqlite::{Connection, OpenFlags, OptionalExtension, types::Value},
};
use sha2::{Digest, Sha256};
use std::{io, path::Path};

pub(super) fn run(world: &Path, database: &Database, conversation: &str) -> io::Result<()> {
    import(world, database, conversation).map_err(io::Error::other)
}
fn import(world: &Path, database: &Database, conversation: &str) -> Result<()> {
    let path = path(world, conversation)?;
    if !disk::exists(&path).map_err(|error| Error::new(error.to_string()))? {
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
        return Err(Error::new("an unfinished conversation import is missing its bundle"));
    }
    let source = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.execute_batch("PRAGMA trusted_schema=OFF; BEGIN")?;
    let tables = database.read_background_now(schema::tables)?;
    let manifest = bundle::manifest(&source, conversation, &tables)?;
    let fingerprint =
        format!("{:x}", Sha256::digest(serde_json::to_vec(&manifest).map_err(|error| Error::new(error.to_string()))?));
    let (scope, signature) = (conversation.to_string(), fingerprint.clone());
    database.write_now_at(scheduler::priority(), 256, move |db| {
        super::super::check(db, &scope)?;
        let prior: Option<String> =
            db.query_row("SELECT fingerprint FROM node_conversation_transfers WHERE name=?1", [&scope], |row| {
                row.get(0)
            })?;
        if prior.as_ref().is_some_and(|prior| prior != &signature) {
            return Err(Error::new("the pending conversation bundle was replaced"));
        }
        db.execute("UPDATE node_conversation_transfers SET fingerprint=?2 WHERE name=?1", (&scope, signature))?;
        Ok(())
    })?;
    for proof in &manifest.tables {
        scheduler::checkpoint().map_err(|error| Error::new(error.to_string()))?;
        bundle::verify(&source, conversation, proof)?;
    }
    let mut order = database.read_background_now(|db| schema::children_first(db, tables))?;
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
            scheduler::checkpoint().map_err(|error| Error::new(error.to_string()))?;
            let batch = bundle::page(&source, conversation, &table, &after)?;
            let Some(last) = batch.last() else {
                break;
            };
            after = bundle::key(&table, last)?;
            let cursor = encode(&after)?;
            let bytes = batch.iter().flatten().map(bundle::value_bytes).sum::<usize>() + cursor.len();
            let (table, scope) = (table.clone(), conversation.to_string());
            database.write_now_at(scheduler::priority(), bytes, move |db| {
                super::super::check(db, &scope)?;
                let _triggers = super::triggers::SuspendedTriggers::new(db)?;
                for row in batch {
                    bundle::put(db, &table, &row)?;
                }
                db.execute(
                    "INSERT INTO node_conversation_transfer_progress(name,table_name,cursor) VALUES(?1,?2,?3)
                    ON CONFLICT(name,table_name) DO UPDATE SET cursor=excluded.cursor",
                    (&scope, &table.name, cursor),
                )?;
                Ok(())
            })?;
        }
    }
    for proof in &manifest.tables {
        scheduler::checkpoint().map_err(|error| Error::new(error.to_string()))?;
        database.read_background_now(|db| bundle::verify(db, conversation, proof))?;
    }
    let scope = conversation.to_string();
    database.write_now_at(Priority::Background, 128, move |db| {
        super::super::check(db, &scope)?;
        db.execute("UPDATE node_conversation_transfers SET finished=1 WHERE name=?1", [&scope])?;
        Ok(())
    })?;
    drop(source);
    disk::remove_file(path).map_err(|error| Error::new(error.to_string()))?;
    complete(world, database, conversation)
}

fn complete(world: &Path, database: &Database, conversation: &str) -> Result<()> {
    let scope = conversation.to_string();
    database.write_now_at(Priority::Background, 128, move |db| {
        db.execute("DELETE FROM node_conversation_transfer_progress WHERE name=?1", [&scope])?;
        db.execute("DELETE FROM node_conversation_transfers WHERE name=?1", [&scope])?;
        Ok(())
    })?;
    if let Some(pending) = PENDING.lock().expect("conversation transfers").get_mut(world) {
        pending.remove(conversation);
    }
    fastctx_platform::readings::counter("storage.conversations_imported").add(1);
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(String),
    Blob(Vec<u8>),
}
fn encode(values: &[Value]) -> Result<Vec<u8>> {
    let cells: Vec<_> = values
        .iter()
        .map(|value| match value {
            Value::Null => Cell::Null,
            Value::Integer(value) => Cell::Integer(*value),
            Value::Real(value) => Cell::Real(value.to_bits()),
            Value::Text(value) => Cell::Text(value.clone()),
            Value::Blob(value) => Cell::Blob(value.clone()),
        })
        .collect();
    serde_json::to_vec(&cells).map_err(|error| Error::new(error.to_string()))
}
fn decode(bytes: &[u8]) -> Result<Vec<Value>> {
    let cells: Vec<Cell> = serde_json::from_slice(bytes).map_err(|error| Error::new(error.to_string()))?;
    Ok(cells
        .into_iter()
        .map(|cell| match cell {
            Cell::Null => Value::Null,
            Cell::Integer(value) => Value::Integer(value),
            Cell::Real(value) => Value::Real(f64::from_bits(value)),
            Cell::Text(value) => Value::Text(value),
            Cell::Blob(value) => Value::Blob(value),
        })
        .collect())
}
