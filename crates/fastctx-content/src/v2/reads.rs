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
use fastctx_storage::rusqlite::{OptionalExtension, params};

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
}

/// The most entries one page gives.
pub const PAGE: usize = 500;

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
        },
        row.get::<_, Option<Vec<u8>>>(6)?.unwrap_or_default(),
    ))
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
            let mut statement = db.prepare_cached(
                "SELECT i.item, i.place, i.revision, i.kind, i.schema, i.body, i.objects FROM items i
                 JOIN streams s ON s.id = i.stream
                 WHERE s.scope = ?1 AND s.name = ?2 AND i.holding = ?3 AND i.state = ?4
                   AND (?5 = 0 OR (i.place, i.item) > (?6, ?7))
                 ORDER BY i.place, i.item LIMIT ?8",
            )?;
            let started = after.is_some();
            let (place, item) = after.unwrap_or_default();
            let rows = statement.query_map(params![scope, stream, holding, IS, started, place, item, limit], entry)?;
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
                    "SELECT i.item, i.place, i.revision, i.kind, i.schema, i.body, i.objects FROM items i
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

    /// Reads `hash` within `scope`, through the gate.
    pub fn read_in(&self, scope: &str, hash: &[u8; 32], offset: u64, length: u32) -> Result<Vec<u8>> {
        if !self.readable(scope, hash)? {
            return Err(Error::Invalid("the object is not readable in that scope".into()));
        }
        self.read(hash, offset, length)
    }
}
