//! Indexed rows of the hub's durable state. Runtime writes all use the
//! service's one batching writer; startup imports on its opening transaction.

use super::rows::Changes;
use fastctx_storage::{
    Database, Error,
    rusqlite::{Connection, OptionalExtension, params},
};
use serde::de::DeserializeOwned;
use std::{io, path::Path};

pub(crate) const FILE: &str = "hub.sqlite3";
pub(crate) const WAL: &str = "hub.sqlite3-wal";
pub(crate) const SHM: &str = "hub.sqlite3-shm";
pub(crate) const JOURNAL: &str = "hub.sqlite3-journal";
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS hub_rows (
    kind TEXT NOT NULL, key TEXT NOT NULL, position INTEGER NOT NULL, body BLOB NOT NULL,
    PRIMARY KEY (kind, key)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS hub_rows_in_order ON hub_rows(kind, position);
CREATE TABLE IF NOT EXISTS hub_meta (name TEXT PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS machine_log (
    id INTEGER PRIMARY KEY, at INTEGER NOT NULL, nano INTEGER NOT NULL,
    needs INTEGER NOT NULL, record BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS machine_log_by_time ON machine_log(at, nano);
CREATE TABLE IF NOT EXISTS machine_log_readers (
    reader TEXT NOT NULL, id INTEGER NOT NULL REFERENCES machine_log(id) ON DELETE CASCADE,
    PRIMARY KEY (reader, id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS machine_log_readers_by_record ON machine_log_readers(id);
CREATE TABLE IF NOT EXISTS machine_log_notified (
    reader TEXT NOT NULL, id INTEGER NOT NULL REFERENCES machine_log(id) ON DELETE CASCADE,
    PRIMARY KEY (reader, id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS machine_log_notified_by_record ON machine_log_notified(id);
CREATE TABLE IF NOT EXISTS machine_log_updated (
    machine TEXT NOT NULL, was TEXT NOT NULL, now TEXT NOT NULL,
    id INTEGER NOT NULL REFERENCES machine_log(id) ON DELETE CASCADE,
    PRIMARY KEY(machine, was, now, id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS machine_log_updated_by_record ON machine_log_updated(id);
"#;

#[derive(Clone, Debug)]
pub(crate) struct Shelf(pub(crate) Database);

impl Shelf {
    pub(crate) fn open(dir: &Path) -> io::Result<Self> {
        if !fastctx_platform::disk::exists(dir.join(FILE))? {
            for name in ["hub.json", "directory.json"] {
                if !fastctx_platform::disk::exists(dir.join(name))? {
                    return Err(io::Error::new(io::ErrorKind::NotFound, format!("the hub's {name} is missing")));
                }
            }
        }
        super::format::prepare(dir)?;
        let database =
            Database::open_initialized(&dir.join(FILE), SCHEMA, |connection| super::migrate::import(connection, dir))
                .map_err(other)?;
        if let Some(report) = database.salvaged() {
            eprintln!("the hub's database was damaged and set aside; what could be read was salvaged:\n{report}");
        }
        let shelf = Self(database);
        super::migrate::back_up(&shelf, dir)?;
        Ok(shelf)
    }

    pub(crate) fn meta<T: DeserializeOwned>(&self, name: &str) -> io::Result<Option<T>> {
        self.0.read_now(|c| meta(c, name)).map_err(other)
    }

    pub(crate) fn rows<T: DeserializeOwned>(&self, kind: &str) -> io::Result<Vec<(String, i64, T)>> {
        self.0
            .read_now(|c| {
                let mut statement = c.prepare_cached(
                    "SELECT key, position, body FROM hub_rows WHERE kind = ?1 ORDER BY position, key",
                )?;
                let mut cursor = statement.query([kind])?;
                let mut found = Vec::new();
                while let Some(row) = cursor.next()? {
                    let bytes: Vec<u8> = row.get(2)?;
                    found.push((row.get(0)?, row.get(1)?, decode(&bytes)?));
                }
                Ok(found)
            })
            .map_err(other)
    }

    pub(crate) async fn write_async(&self, changes: Changes) -> io::Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        self.0.write(changes.bytes(), move |c| apply(c, &changes)).await.map_err(other)
    }

    pub(crate) fn write(&self, changes: Changes) -> io::Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        self.0.write_now(changes.bytes(), move |c| apply(c, &changes)).map_err(other)
    }
}

pub(crate) fn meta<T: DeserializeOwned>(c: &Connection, name: &str) -> fastctx_storage::Result<Option<T>> {
    let bytes: Option<Vec<u8>> =
        c.prepare_cached("SELECT value FROM hub_meta WHERE name = ?1")?.query_row([name], |r| r.get(0)).optional()?;
    bytes.map(|b| decode(&b)).transpose()
}

pub(crate) fn put_meta(c: &Connection, name: &str, value: &[u8]) -> fastctx_storage::Result<()> {
    c.prepare_cached(
        "INSERT INTO hub_meta(name,value) VALUES (?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",
    )?
    .execute(params![name, value])?;
    Ok(())
}

pub(crate) fn put(c: &Connection, kind: &str, key: &str, position: i64, body: &[u8]) -> fastctx_storage::Result<()> {
    c.prepare_cached("INSERT INTO hub_rows(kind,key,position,body) VALUES (?1,?2,?3,?4) ON CONFLICT(kind,key) DO UPDATE SET position=excluded.position,body=excluded.body")?
        .execute(params![kind,key,position,body])?;
    Ok(())
}

fn apply(c: &Connection, changes: &Changes) -> fastctx_storage::Result<()> {
    reconcile_letters(c, changes)?;
    for kind in &changes.cleared {
        c.prepare_cached("DELETE FROM hub_rows WHERE kind=?1")?.execute([kind])?;
    }
    for (kind, key) in &changes.deleted {
        c.prepare_cached("DELETE FROM hub_rows WHERE kind=?1 AND key=?2")?.execute(params![kind, &**key])?;
    }
    for row in &changes.put {
        put(c, row.kind, &row.key, row.position, &row.body)?;
    }
    for (kind, key, position) in &changes.moved {
        c.prepare_cached("UPDATE hub_rows SET position=?3 WHERE kind=?1 AND key=?2")?
            .execute(params![kind, &**key, position])?;
    }
    for (name, value) in &changes.meta {
        put_meta(c, name, value)?;
    }
    if changes.log_cleared {
        c.execute("DELETE FROM machine_log", [])?;
    }
    if let Some(after) = changes.log_after {
        let after = super::log_store::sql_id(after);
        c.prepare_cached("DELETE FROM machine_log WHERE id>?1")?.execute([after])?;
    }
    for row in &changes.log {
        super::log_store::put(c, row)?;
    }
    if let Some(before) = changes.expired {
        super::log_store::expire(c, before)?;
    }
    Ok(())
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> fastctx_storage::Result<T> {
    serde_json::from_slice(bytes).map_err(|e| Error::new(e.to_string()))
}
pub(crate) fn other(error: Error) -> io::Error {
    io::Error::other(error.to_string())
}

/// Catalog renames and removals change the durable mail row in the same
/// transaction. File cleanup can lag without assigning an old epoch to a
/// replacement identity on restart. Renames retain the outbox epoch.
fn reconcile_letters(c: &Connection, changes: &Changes) -> fastctx_storage::Result<()> {
    let mut removed = Vec::new();
    for (_, key) in changes.deleted.iter().filter(|(kind, _)| *kind == "members") {
        let before: Option<Vec<u8>> = c
            .query_row("SELECT body FROM hub_rows WHERE kind='members' AND key=?1", [&**key], |row| row.get(0))
            .optional()?;
        if let Some(before) = before {
            removed.push(decode::<fastctx_net::directory::Member>(&before)?.name);
        }
    }
    if changes.cleared.contains(&"members") {
        let kept: std::collections::HashSet<_> =
            changes.put.iter().filter(|row| row.kind == "members").map(|row| &*row.key).collect();
        let mut query = c.prepare("SELECT key, body FROM hub_rows WHERE kind='members'")?;
        let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))?;
        for row in rows {
            let (key, bytes) = row?;
            if !kept.contains(key.as_str()) {
                removed.push(decode::<fastctx_net::directory::Member>(&bytes)?.name);
            }
        }
    }
    for name in removed {
        c.execute("DELETE FROM hub_rows WHERE kind='letters' AND key=?1", [name.as_str()])?;
    }
    let mut moving = Vec::new();
    for row in changes.put.iter().filter(|row| row.kind == "members") {
        let before: Option<Vec<u8>> = c
            .query_row("SELECT body FROM hub_rows WHERE kind='members' AND key=?1", [&*row.key], |row| row.get(0))
            .optional()?;
        let Some(before) = before else { continue };
        let before = decode::<fastctx_net::directory::Member>(&before)?;
        let after = decode::<fastctx_net::directory::Member>(&row.body)?;
        if before.name.as_str() == after.name.as_str() {
            continue;
        }
        let bytes: Option<Vec<u8>> = c
            .query_row("SELECT body FROM hub_rows WHERE kind='letters' AND key=?1", [before.name.as_str()], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(bytes) = bytes {
            moving.push((before.name, after.name, bytes));
        }
    }
    for (before, _, _) in &moving {
        c.execute("DELETE FROM hub_rows WHERE kind='letters' AND key=?1", [before.as_str()])?;
    }
    for (_, after, bytes) in moving {
        put(c, "letters", after.as_str(), 0, &bytes)?;
    }
    Ok(())
}
