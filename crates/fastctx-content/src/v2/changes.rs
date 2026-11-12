//! What changed in a hub's owner library, by the library's own change
//! number (design §28.1): each stream's latest change is one row, so a
//! device asks what changed after where it stands and the answer costs only
//! what changed, however long it was away. A library's generation is new
//! each time the hub reopens it after being put back (§17): a device whose
//! number is of another generation starts again from nothing.
//!
//! On a device, where it stands in each owner library of each hub.

use super::Library;
use super::objects::now;
use crate::Result;
use crate::error::invalid;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

/// The stamp the library's generation is kept under.
const GENERATION: &str = "generation";
/// The stamp the library's latest change number is kept under.
const CHANGE: &str = "change";

/// One stream's latest change in an owner library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    pub change: u64,
    pub scope: String,
    pub stream: String,
    /// The sequence the stream's next frame takes.
    pub next: u64,
    pub head: [u8; 32],
    /// The bytes of all the stream's frames through its head.
    pub bytes: u64,
    /// The change at which the hub last indexed the stream's records again,
    /// kept as later frames move it (E08); 0 when it never did.
    pub remade: u64,
    /// When it changed, in the hub's milliseconds.
    pub at: i64,
}

/// One page of what changed, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changes {
    pub generation: String,
    pub rows: Vec<Changed>,
    /// The library's latest change number when the first page was asked,
    /// and when that change was.
    pub top: u64,
    pub top_at: i64,
    /// Where the next page goes on below; `after` once there is no more.
    pub through: u64,
    pub more: bool,
}

/// The class of a stream, by its name: the order streams are caught up in
/// after being away, small state first (§28.1).
pub(super) fn class_of(stream: &str) -> u8 {
    if stream.starts_with("timeline") {
        2
    } else if stream.starts_with("output") || stream.starts_with("objects") {
        3
    } else {
        1
    }
}

/// A new generation: different from every other, nothing more.
fn fresh_generation() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut hasher = Sha256::new();
    hasher.update(b"fastctx-library-generation/1");
    hasher.update(now().to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_le_bytes());
    if let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.update(elapsed.as_nanos().to_le_bytes());
    }
    hasher.finalize()[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `stream` of a hub's library moved by a frame of `bytes` bytes, or was
/// indexed again (`remade`, `bytes` 0): its row takes the library's next
/// change number, with the stream's head as it stands; indexed again, the
/// number is kept as where it was last remade, which later frames leave be
/// (design §28.1). In the transaction that moved it.
pub(super) fn moved(db: &Connection, stream: i64, bytes: u64, remade: bool) -> fastctx_storage::Result<()> {
    let (name, next, head): (String, i64, Option<Vec<u8>>) = db
        .prepare_cached("SELECT name, next, head FROM streams WHERE id = ?1")?
        .query_row([stream], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let latest: i64 = db
        .prepare_cached("SELECT value FROM stamps WHERE name = ?1")?
        .query_row([CHANGE], |row| row.get::<_, Vec<u8>>(0))
        .optional()?
        .and_then(|value| <[u8; 8]>::try_from(value.as_slice()).ok().map(i64::from_le_bytes))
        .unwrap_or(0);
    let change = latest + 1;
    db.prepare_cached(
        "INSERT INTO stamps (name, value) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET value = excluded.value",
    )?
    .execute(params![CHANGE, change.to_le_bytes().to_vec()])?;
    let (before, was_remade): (i64, i64) = db
        .prepare_cached("SELECT bytes, remade FROM stream_changes WHERE stream = ?1")?
        .query_row([stream], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?
        .unwrap_or((0, 0));
    db.prepare_cached(
        "INSERT INTO stream_changes (stream, change, class, next, head, bytes, remade, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (stream) DO UPDATE SET change = excluded.change, class = excluded.class, next = excluded.next,
         head = excluded.head, bytes = excluded.bytes, remade = excluded.remade, at = excluded.at",
    )?
    .execute(params![
        stream,
        change,
        i64::from(class_of(&name)),
        next,
        head.unwrap_or_else(|| vec![0; 32]),
        before + bytes as i64,
        if remade { change } else { was_remade },
        now()
    ])?;
    Ok(())
}

impl Library {
    /// The library's generation, made the first time it is asked.
    pub fn generation(&self) -> Result<String> {
        let value = match self.stamped(GENERATION)? {
            Some(value) => value,
            None => self.stamp(GENERATION, fresh_generation().into_bytes())?,
        };
        String::from_utf8(value).map_err(|_| crate::Error::Invalid("a malformed library generation".into()))
    }

    /// A new generation for the library, as the hub reopens it after being
    /// put back: devices that stood in the one before start again.
    pub fn renew_generation(&self) -> Result<String> {
        let generation = fresh_generation();
        let value = generation.clone().into_bytes();
        self.database().write_now_at(Priority::Control, 128, move |db| {
            db.prepare_cached("INSERT INTO stamps (name, value) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET value = excluded.value")?
                .execute(params![GENERATION, value])?;
            Ok(())
        })?;
        Ok(generation)
    }

    /// Up to `limit` streams of the hub's library whose latest change is
    /// after `after` and below `before` (or the latest, when none), of
    /// `class` alone when given, newest first.
    pub fn changes(&self, class: Option<u8>, after: u64, before: Option<u64>, limit: usize) -> Result<Changes> {
        let generation = self.generation()?;
        Ok(self.database().read_now(move |db| {
            let (top, top_at): (i64, i64) = db
                .prepare_cached("SELECT change, at FROM stream_changes ORDER BY change DESC LIMIT 1")?
                .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?
                .unwrap_or((0, 0));
            let below = before.map_or(top + 1, |before| before as i64);
            let class = class.map(i64::from);
            let mut statement = db.prepare_cached(
                "SELECT c.change, s.key, t.name, c.next, c.head, c.bytes, c.remade, c.at FROM stream_changes c
                 JOIN streams t ON t.id = c.stream JOIN scopes s ON s.id = t.scope
                 WHERE c.change > ?1 AND c.change < ?2 AND (?3 IS NULL OR c.class = ?3)
                 ORDER BY c.change DESC LIMIT ?4",
            )?;
            let rows = statement.query_map(params![after as i64, below, class, limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get(7)?,
                ))
            })?;
            let mut changed = Vec::new();
            for row in rows {
                let (change, scope, stream, next, head, bytes, remade, at) = row?;
                let head: [u8; 32] = head.try_into().map_err(|_| invalid("a malformed stream head"))?;
                changed.push(Changed {
                    change: change as u64,
                    scope,
                    stream,
                    next: next as u64,
                    head,
                    bytes: bytes as u64,
                    remade: remade as u64,
                    at,
                });
            }
            let more = changed.len() == limit && limit > 0;
            let through = changed.last().map_or(after, |last| last.change);
            Ok(Changes { generation, rows: changed, top: top as u64, top_at, through, more })
        })?)
    }

    /// Where this device stands in `owner`'s library of `hub`: its
    /// generation and the change number it caught up to.
    pub fn feed(&self, hub: &str, owner: &str) -> Result<Option<(String, u64)>> {
        let (hub, owner) = (hub.to_string(), owner.to_string());
        Ok(self.database().read_now(move |db| {
            Ok(db
                .prepare_cached("SELECT generation, change FROM feeds WHERE hub = ?1 AND owner = ?2")?
                .query_row([&hub, &owner], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)))
                .optional()?)
        })?)
    }

    /// This device caught up to `change` of `owner`'s library of `hub` in
    /// `generation`, a change made at `at`.
    pub fn fed(&self, hub: &str, owner: &str, generation: &str, change: u64, at: i64) -> Result<()> {
        let (hub, owner, generation) = (hub.to_string(), owner.to_string(), generation.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 256, move |db| {
            db.prepare_cached(
                "INSERT INTO feeds (hub, owner, generation, change, at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (hub, owner) DO UPDATE SET generation = excluded.generation, change = excluded.change, at = excluded.at",
            )?
            .execute(params![hub, owner, generation, change as i64, at])?;
            Ok(())
        })?)
    }

    /// The owner libraries of `hub` this device has stood in.
    pub fn feed_owners(&self, hub: &str) -> Result<Vec<String>> {
        let hub = hub.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached("SELECT owner FROM feeds WHERE hub = ?1 ORDER BY owner")?;
            let rows = statement.query_map([&hub], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Forgets where this device stood in every owner library of `hub`: it
    /// left the network, and a later joining starts again from nothing.
    pub fn forget_feeds(&self, hub: &str) -> Result<()> {
        let hub = hub.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 128, move |db| {
            db.prepare_cached("DELETE FROM feeds WHERE hub = ?1")?.execute([&hub])?;
            Ok(())
        })?)
    }
}
