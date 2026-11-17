//! Reading the index and the local read gate (§4.5, §6.4).
//!
//! Only the current holding period's rows are read: an ended period's rows
//! and floors are still on disk until purged, and never answer. A page is
//! one range of a stream's order. An object is readable in a scope when the
//! current period's index refers to it or one of the period's applied
//! frames does; a parked or rejected frame's reference keeps bytes alive
//! but gives no reading (P01).

use super::Library;
use super::items::IS;
use super::scopes::held;
use crate::record::{ObjectRef, unpack};
use crate::{Error, Result};
use fastctx_storage::rusqlite::types::Value;
use fastctx_storage::rusqlite::{OptionalExtension, params, params_from_iter};

/// An item as the index has it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub item: Vec<u8>,
    pub order: Vec<u8>,
    pub revision: u64,
    pub kind: u16,
    pub schema: u16,
    pub body: Vec<u8>,
    pub objects: Vec<ObjectRef>,
    /// The write number the row took here (`items.rs`): what a reader on
    /// this device knows it by; none where it is not read from the index.
    pub local: u64,
}

/// The most entries one page gives.
pub const PAGE: usize = 500;

/// An item as `Library::orders` lists it: its identity, its order key and
/// the length of its body.
pub type Placed = (Vec<u8>, Vec<u8>, u64);

fn entry(row: &fastctx_storage::rusqlite::Row<'_>) -> fastctx_storage::rusqlite::Result<(Entry, Vec<u8>)> {
    Ok((
        Entry {
            item: row.get(0)?,
            order: row.get(1)?,
            revision: row.get::<_, i64>(2)? as u64,
            kind: row.get(3)?,
            schema: row.get(4)?,
            body: row.get::<_, Option<Vec<u8>>>(5)?.unwrap_or_default(),
            objects: Vec::new(),
            local: row.get::<_, i64>(7)? as u64,
        },
        row.get::<_, Option<Vec<u8>>>(6)?.unwrap_or_default(),
    ))
}

/// A page's query: `head` (its parameters ?1 to ?4), past the entry where
/// the page before ended (its order key and identity, ?6 and ?7) — `beyond`
/// it — where there was one, then `tail` (the limit, ?5); and what it binds.
/// Two statements rather than one that asks whether to go on: SQLite seeks
/// the page's start in the items' index only where the bound is a plain
/// one, and otherwise walks to it from the first item, every page of a long
/// stream again.
fn seek(head: &str, from: Option<(Vec<u8>, Vec<u8>)>, beyond: &str, tail: &str, bound: [Value; 5]) -> (String, Vec<Value>) {
    let mut values = bound.to_vec();
    match from {
        Some((place, item)) => {
            values.extend([Value::Blob(place), Value::Blob(item)]);
            (format!("{head} AND (i.place, i.item) {beyond} (?6, ?7) {tail}"), values)
        }
        None => (format!("{head} {tail}"), values),
    }
}

impl Library {
    /// Up to `limit` live items of `stream` in order, from the current
    /// holding period: from the first, or after the entry `after` (its order
    /// key and identity, as the last page ended). Order keys may be empty
    /// and may repeat; the identity breaks ties.
    pub fn page(&self, scope: &str, stream: &str, after: Option<(Vec<u8>, Vec<u8>)>, limit: usize) -> Result<Vec<Entry>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        let limit = limit.min(PAGE) as i64;
        Ok(self.database().read_now(move |db| {
            let Some((scope, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            // A page after another sought where it begins in the index, not
            // walked to from the first (`seek`).
            let (query, values) = seek(
                "SELECT i.item, i.place, i.revision, i.kind, i.schema, i.body, i.objects, i.local FROM items i
                 JOIN streams s ON s.id = i.stream
                 WHERE s.scope = ?1 AND s.name = ?2 AND i.holding = ?3 AND i.state = ?4",
                after,
                ">",
                "ORDER BY i.place, i.item LIMIT ?5",
                [Value::Integer(scope), Value::Text(stream), Value::Integer(holding), Value::Integer(IS), Value::Integer(limit)],
            );
            let mut statement = db.prepare_cached(&query)?;
            let rows = statement.query_map(params_from_iter(values), entry)?;
            let mut entries = Vec::new();
            for row in rows {
                let (mut entry, objects) = row?;
                entry.objects = unpack(&objects).map_err(crate::error::invalid)?;
                entries.push(entry);
            }
            Ok(entries)
        })?)
    }

    /// What `page` lists without the bodies: each live item's identity, its
    /// order key, and the length of its body — for a reading too large to
    /// hold whole, which takes the bodies it compares a few at a time
    /// (`items`).
    pub fn orders(&self, scope: &str, stream: &str, after: Option<(Vec<u8>, Vec<u8>)>, limit: usize) -> Result<Vec<Placed>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        let limit = limit.min(PAGE) as i64;
        Ok(self.database().read_now(move |db| {
            let Some((scope, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            let (query, values) = seek(
                "SELECT i.item, i.place, length(i.body) FROM items i
                 JOIN streams s ON s.id = i.stream
                 WHERE s.scope = ?1 AND s.name = ?2 AND i.holding = ?3 AND i.state = ?4",
                after,
                ">",
                "ORDER BY i.place, i.item LIMIT ?5",
                [Value::Integer(scope), Value::Text(stream), Value::Integer(holding), Value::Integer(IS), Value::Integer(limit)],
            );
            let mut statement = db.prepare_cached(&query)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Option<i64>>(2)?.unwrap_or(0).max(0) as u64))
            })?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?)
    }

    /// What `orders` lists of `items` alone, those `stream` holds live now
    /// in the current holding period: each one's identity, order key and
    /// the length of its body — read at once in one snapshot, as `items`
    /// reads them, without the bodies.
    pub fn orders_of(&self, scope: &str, stream: &str, items: &[Vec<u8>]) -> Result<Vec<Placed>> {
        let (scope, stream, items) = (scope.to_string(), stream.to_string(), items.to_vec());
        Ok(self.database().read_now(move |connection| {
            let db = connection.unchecked_transaction()?;
            let mut found = Vec::new();
            let Some((scope, _, Some(holding))) = held(&db, &scope)? else { return Ok(found) };
            let mut statement = db.prepare_cached(
                "SELECT i.place, length(i.body) FROM items i
                 JOIN streams s ON s.id = i.stream
                 WHERE s.scope = ?1 AND s.name = ?2 AND i.item = ?3 AND i.holding = ?4 AND i.state = ?5",
            )?;
            for item in items {
                let row = statement
                    .query_row(params![scope, stream, item, holding, IS], |row| {
                        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Option<i64>>(1)?.unwrap_or(0).max(0) as u64))
                    })
                    .optional()?;
                if let Some((place, length)) = row {
                    found.push((item, place, length));
                }
            }
            Ok(found)
        })?)
    }

    /// Up to `limit` live items of `stream` in reverse order, from the
    /// current holding period: from the last, or before the entry `before`
    /// (its order key and identity, as the last page ended).
    pub fn page_before(&self, scope: &str, stream: &str, before: Option<(Vec<u8>, Vec<u8>)>, limit: usize) -> Result<Vec<Entry>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        let limit = limit.min(PAGE) as i64;
        Ok(self.database().read_now(move |db| {
            let Some((scope, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            let (query, values) = seek(
                "SELECT i.item, i.place, i.revision, i.kind, i.schema, i.body, i.objects, i.local FROM items i
                 JOIN streams s ON s.id = i.stream
                 WHERE s.scope = ?1 AND s.name = ?2 AND i.holding = ?3 AND i.state = ?4",
                before,
                "<",
                "ORDER BY i.place DESC, i.item DESC LIMIT ?5",
                [Value::Integer(scope), Value::Text(stream), Value::Integer(holding), Value::Integer(IS), Value::Integer(limit)],
            );
            let mut statement = db.prepare_cached(&query)?;
            let rows = statement.query_map(params_from_iter(values), entry)?;
            let mut entries = Vec::new();
            for row in rows {
                let (mut entry, objects) = row?;
                entry.objects = unpack(&objects).map_err(crate::error::invalid)?;
                entries.push(entry);
            }
            Ok(entries)
        })?)
    }

    /// One item of `stream` as the current period's index has it; `None`
    /// when it is not there or was deleted.
    pub fn item(&self, scope: &str, stream: &str, item: &[u8]) -> Result<Option<Entry>> {
        let (scope, stream, item) = (scope.to_string(), stream.to_string(), item.to_vec());
        Ok(self.database().read_now(move |db| {
            let Some((scope, _, Some(holding))) = held(db, &scope)? else { return Ok(None) };
            let found = db
                .prepare_cached(
                    "SELECT i.item, i.place, i.revision, i.kind, i.schema, i.body, i.objects, i.local FROM items i
                     JOIN streams s ON s.id = i.stream
                     WHERE s.scope = ?1 AND s.name = ?2 AND i.item = ?3 AND i.holding = ?4 AND i.state = ?5",
                )?
                .query_row(params![scope, stream, item, holding, IS], entry)
                .optional()?;
            match found {
                Some((mut entry, objects)) => {
                    entry.objects = unpack(&objects).map_err(crate::error::invalid)?;
                    Ok(Some(entry))
                }
                None => Ok(None),
            }
        })?)
    }

    /// The entries of `items` in `stream` of `scope` as they stand, each as
    /// `item` gives it, read at once in one snapshot: those it holds.
    pub fn items(&self, scope: &str, stream: &str, items: &[Vec<u8>]) -> Result<std::collections::HashMap<Vec<u8>, Entry>> {
        let (scope, stream, items) = (scope.to_string(), stream.to_string(), items.to_vec());
        Ok(self.database().read_now(move |connection| {
            // The holding checked and every item read in one read transaction:
            // a holding ended meanwhile reads none of the rows it leaves.
            let db = connection.unchecked_transaction()?;
            let mut found = std::collections::HashMap::new();
            let Some((scope, _, Some(holding))) = held(&db, &scope)? else { return Ok(found) };
            let mut statement = db.prepare_cached(
                "SELECT i.item, i.place, i.revision, i.kind, i.schema, i.body, i.objects, i.local FROM items i
                 JOIN streams s ON s.id = i.stream
                 WHERE s.scope = ?1 AND s.name = ?2 AND i.item = ?3 AND i.holding = ?4 AND i.state = ?5",
            )?;
            for item in items {
                if let Some((mut entry, objects)) = statement.query_row(params![scope, stream, item, holding, IS], entry).optional()? {
                    entry.objects = unpack(&objects).map_err(crate::error::invalid)?;
                    found.insert(item, entry);
                }
            }
            Ok(found)
        })?)
    }

    /// Which of `items` the index of `stream` of `scope` has a row of now,
    /// whatever its state or period — gone (a tombstone) too. Not the items
    /// it ever had: rows a holding period's end or a floor took out of the
    /// index are not among them.
    pub fn had_items(&self, scope: &str, stream: &str, items: &[Vec<u8>]) -> Result<std::collections::HashSet<Vec<u8>>> {
        let (scope, stream, items) = (scope.to_string(), stream.to_string(), items.to_vec());
        Ok(self.database().read_now(move |db| {
            let mut had = std::collections::HashSet::new();
            let Some((scope, _, _)) = held(db, &scope)? else { return Ok(had) };
            let mut query = db.prepare_cached(
                "SELECT 1 FROM items i JOIN streams s ON s.id = i.stream WHERE s.scope = ?1 AND s.name = ?2 AND i.item = ?3",
            )?;
            for item in items {
                if query.query_row(params![scope, stream, item], |_| Ok(())).optional()?.is_some() {
                    had.insert(item);
                }
            }
            Ok(had)
        })?)
    }

    /// Whether `hash` may be read in `scope` here: the current period's
    /// index or one of its applied frames refers to it.
    pub fn readable(&self, scope: &str, hash: &[u8; 32]) -> Result<bool> {
        let (scope, hash) = (scope.to_string(), *hash);
        Ok(self.database().read_now(move |db| {
            let Some((scope, state, Some(holding))) = held(db, &scope)? else { return Ok(false) };
            if state != "live" && state != "provisional" {
                return Ok(false);
            }
            let referred = db
                .prepare_cached(
                    "SELECT 1 FROM object_refs WHERE scope = ?1 AND holding = ?2 AND hash = ?3 AND count > 0
                     UNION ALL
                     SELECT 1 FROM frame_objects f JOIN streams s ON s.id = f.stream
                     WHERE s.scope = ?1 AND f.holding = ?2 AND f.hash = ?3
                     LIMIT 1",
                )?
                .query_row(params![scope, holding, &hash[..]], |_| Ok(()))
                .optional()?
                .is_some();
            Ok(referred)
        })?)
    }

    /// The streams of `scope` through which `hash` may be read here: those
    /// whose index or applied frames refer to it in the current period, and
    /// those whose evidence refers to it with its holding shown (§2.3: a
    /// device reads an object only through a stream it may hold).
    pub fn streams_reading(&self, scope: &str, hash: &[u8; 32]) -> Result<Vec<String>> {
        let (scope, hash) = (scope.to_string(), *hash);
        Ok(self.database().read_now(move |db| {
            let Some((scope, state, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            if state != "live" {
                return Ok(Vec::new());
            }
            let mut statement = db.prepare_cached(
                "SELECT s.name FROM object_refs r JOIN streams s ON s.id = r.stream
                 WHERE r.scope = ?1 AND r.holding = ?2 AND r.hash = ?3 AND r.count > 0
                 UNION
                 SELECT s.name FROM frame_objects f JOIN streams s ON s.id = f.stream
                 WHERE s.scope = ?1 AND f.holding = ?2 AND f.hash = ?3
                 UNION
                 SELECT s.name FROM evidence e JOIN streams s ON s.id = e.stream
                 JOIN object_holds h ON h.kind = 'evidence' AND h.holder = e.holder
                 WHERE s.scope = ?1 AND e.holding = ?2 AND h.hash = ?3 AND h.proven = 1",
            )?;
            let rows = statement.query_map(params![scope, holding, &hash[..]], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Whether `hash` may be read in `scope` here through the evidence
    /// gate (§6.4 item 5): a predecessor's tail kept as evidence refers to
    /// it, and the device that brought it showed it holds it (§8, Q05).
    pub fn readable_as_evidence(&self, scope: &str, hash: &[u8; 32]) -> Result<bool> {
        let (scope, hash) = (scope.to_string(), *hash);
        Ok(self.database().read_now(move |db| {
            let Some((scope, state, Some(holding))) = held(db, &scope)? else { return Ok(false) };
            if state != "live" {
                return Ok(false);
            }
            Ok(db
                .prepare_cached(
                    "SELECT 1 FROM evidence e JOIN streams s ON s.id = e.stream
                     JOIN object_holds h ON h.kind = 'evidence' AND h.holder = e.holder
                     WHERE s.scope = ?1 AND e.holding = ?2 AND h.hash = ?3 AND h.proven = 1 LIMIT 1",
                )?
                .query_row(params![scope, holding, &hash[..]], |_| Ok(()))
                .optional()?
                .is_some())
        })?)
    }

    /// Reads `hash` within `scope`, through the gate.
    pub fn read_in(&self, scope: &str, hash: &[u8; 32], offset: u64, length: u32) -> Result<Vec<u8>> {
        if !self.readable(scope, hash)? {
            return Err(Error::Invalid("the object is not readable in that scope".into()));
        }
        self.read(hash, offset, length)
    }
}
