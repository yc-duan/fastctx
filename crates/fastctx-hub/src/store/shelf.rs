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
        if !dir.join(FILE).try_exists()? {
            for name in ["hub.json", "directory.json"] {
                if !dir.join(name).try_exists()? {
                    return Err(io::Error::new(io::ErrorKind::NotFound, format!("the hub's {name} is missing")));
                }
            }
        }
        let database =
            Database::open_initialized(&dir.join(FILE), SCHEMA, |connection| super::migrate::import(connection, dir))
                .map_err(other)?;
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
