//! What a library says of where syncing stands (design §28.9). On a hub's
//! owner library: the bytes of the frames applied after a change number,
//! stream by stream; the streams of given scopes; what it holds in all. On
//! a device: where it stands in each owner library of a hub, what it lacks
//! of the objects it is to hold, and what it made that is not sent yet.
//! Each read costs what it counts: the frames after a change by their
//! index, a hub's scopes by theirs.

use super::Library;
use super::changes::class_of;
use crate::Result;
use fastctx_storage::rusqlite::OptionalExtension;

/// The bytes of one stream's frames, as both ends count a frame: header,
/// signature and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamBytes {
    pub scope: String,
    pub stream: String,
    /// The stream's class (design §28.1): 2 a timeline, 3 output and
    /// objects, 1 the rest.
    pub class: u8,
    pub bytes: u64,
}

/// What a hub's owner library holds in all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Scopes with a timeline: conversations.
    pub conversations: u64,
    /// Objects present and referenced, and their bytes.
    pub files: u64,
    pub file_bytes: u64,
    /// The bytes of every stream's frames.
    pub record_bytes: u64,
}

/// Where a device stands in one owner library of a hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fed {
    pub owner: String,
    pub generation: String,
    pub change: u64,
    pub at: i64,
}

impl Library {
    /// A hub library's latest change number and when it was made; none
    /// before its first.
    pub fn latest_change(&self) -> Result<Option<(u64, i64)>> {
        Ok(self.database().read_now(|db| {
            Ok(db
                .prepare_cached("SELECT change, at FROM stream_changes ORDER BY change DESC LIMIT 1")?
                .query_row([], |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)?)))
                .optional()?)
        })?)
    }

    /// The bytes of a hub library's frames applied after the library's
    /// change `change`, stream by stream.
    pub fn bytes_after(&self, change: u64) -> Result<Vec<StreamBytes>> {
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT c.key, s.name, SUM(length(f.header) + length(f.signature) + length(f.body))
                 FROM frames f JOIN streams s ON s.id = f.stream JOIN scopes c ON c.id = s.scope
                 WHERE f.feed > ?1 GROUP BY f.stream",
            )?;
            let rows = statement.query_map([change as i64], |row| {
                let stream: String = row.get(1)?;
                Ok(StreamBytes {
                    scope: row.get(0)?,
                    class: class_of(&stream),
                    stream,
                    bytes: row.get::<_, i64>(2)?.max(0) as u64,
                })
            })?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// The bytes of every stream of each of `scopes` in a hub library, as
    /// far as its latest frame.
    pub fn bytes_of_scopes(&self, scopes: Vec<String>) -> Result<Vec<StreamBytes>> {
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT s.name, c.bytes FROM scopes k JOIN streams s ON s.scope = k.id
                 JOIN stream_changes c ON c.stream = s.id WHERE k.key = ?1",
            )?;
            let mut found = Vec::new();
            for scope in scopes {
                let rows = statement.query_map([&scope], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
                for row in rows {
                    let (stream, bytes) = row?;
                    found.push(StreamBytes { scope: scope.clone(), class: class_of(&stream), stream, bytes: bytes.max(0) as u64 });
                }
            }
            Ok(found)
        })?)
    }

    /// What a hub library holds in all.
    pub fn totals(&self) -> Result<Totals> {
        Ok(self.database().read_now(|db| {
            let conversations: i64 = db
                .prepare_cached("SELECT COUNT(DISTINCT scope) FROM streams WHERE name LIKE 'timeline%'")?
                .query_row([], |row| row.get(0))?;
            let (files, file_bytes): (i64, i64) = db
                .prepare_cached("SELECT COUNT(*), COALESCE(SUM(size), 0) FROM objects WHERE state = 1 AND unreferenced IS NULL")?
                .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            let record_bytes: i64 =
                db.prepare_cached("SELECT COALESCE(SUM(bytes), 0) FROM stream_changes")?.query_row([], |row| row.get(0))?;
            Ok(Totals {
                conversations: conversations.max(0) as u64,
                files: files.max(0) as u64,
                file_bytes: file_bytes.max(0) as u64,
                record_bytes: record_bytes.max(0) as u64,
            })
        })?)
    }

    /// Where this device stands in each owner library of `hub`.
    pub fn feeds_of(&self, hub: &str) -> Result<Vec<Fed>> {
        let hub = hub.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement =
                db.prepare_cached("SELECT owner, generation, change, at FROM feeds WHERE hub = ?1 ORDER BY owner")?;
            let rows = statement.query_map([&hub], |row| {
                Ok(Fed {
                    owner: row.get(0)?,
                    generation: row.get(1)?,
                    change: row.get::<_, i64>(2)? as u64,
                    at: row.get(3)?,
                })
            })?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// The objects this device is to hold of `hub`'s scopes and lacks: how
    /// many, and their bytes.
    pub fn lacking_of(&self, hub: &str) -> Result<(u64, u64)> {
        let hub = hub.to_string();
        Ok(self.database().read_now(move |db| {
            let (count, bytes): (i64, i64) = db
                .prepare_cached(
                    "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM wanted WHERE hash IN
                     (SELECT i.hash FROM scopes s JOIN wanted_in i ON i.scope = s.id WHERE s.hub = ?1)",
                )?
                .query_row([&hub], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok((count.max(0) as u64, bytes.max(0) as u64))
        })?)
    }

    /// The frames this device made in `hub`'s scopes that are not sent yet:
    /// how many, and their bytes.
    pub fn unsent_of(&self, hub: &str) -> Result<(u64, u64)> {
        let hub = hub.to_string();
        Ok(self.database().read_now(move |db| {
            let (count, bytes): (i64, i64) = db
                .prepare_cached(
                    "SELECT COUNT(*), COALESCE(SUM(length(f.header) + length(f.signature) + length(f.body)), 0)
                     FROM scopes c JOIN streams s ON s.scope = c.id JOIN outbox o ON o.stream = s.id
                     JOIN frames f ON f.stream = s.id AND f.first >= o.uploaded WHERE c.hub = ?1",
                )?
                .query_row([&hub], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok((count.max(0) as u64, bytes.max(0) as u64))
        })?)
    }
}
