//! The one way into `items` (§2.4): every path that changes the index —
//! a producer's commit, an imported frame, a re-application after an
//! upgrade, a snapshot, a page, a rebuild — goes through `apply`.
//!
//! A record's revision is its stream sequence number. A row of the current
//! holding period is replaced only by a strictly greater revision; a row of
//! an ended period, or a revision floor its purge left, by a revision at
//! least as great, which moves it into the current period. Lower revisions
//! leave it: a new period refilling from the stream's start does not bring
//! back an item a later revision deleted. A deletion stays as a tombstone.

use super::frame::KindRecord;
use crate::record::{State, pack, unpack};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

pub(super) const IS: i64 = 0;
pub(super) const GONE: i64 = 1;
pub(super) const FLOOR: i64 = 2;

/// Where records are applied: their scope, stream and the scope's current
/// holding period.
#[derive(Clone, Copy, Debug)]
pub(super) struct Applying {
    pub scope: i64,
    pub stream: i64,
    pub holding: i64,
}

/// An index row as it stands: its order key, revision, state, packed
/// object references and holding period.
struct Row {
    place: Vec<u8>,
    revision: i64,
    state: i64,
    objects: Option<Vec<u8>>,
    holding: i64,
}

/// Applies `record` at `revision`; whether it changed the index.
pub(super) fn apply(db: &Connection, at: Applying, revision: u64, record: &KindRecord) -> fastctx_storage::Result<bool> {
    let record_item = &record.record.item;
    let existing = db
        .prepare_cached("SELECT place, revision, state, objects, holding FROM items WHERE stream = ?1 AND item = ?2")?
        .query_row(params![at.stream, record_item], |row| {
            Ok(Row { place: row.get(0)?, revision: row.get(1)?, state: row.get(2)?, objects: row.get(3)?, holding: row.get(4)? })
        })
        .optional()?;
    if let Some(Row { place, revision: known, state, objects, holding }) = existing {
        let current = holding == at.holding && state != FLOOR;
        let replaces = if current { revision as i64 > known } else { revision as i64 >= known };
        if !replaces {
            return Ok(false);
        }
        db.prepare_cached("DELETE FROM items WHERE stream = ?1 AND place = ?2 AND item = ?3")?
            .execute(params![at.stream, place, record_item])?;
        if let Some(objects) = objects {
            let objects = unpack(&objects).map_err(crate::error::invalid)?;
            for object in objects {
                unrefer(db, at.scope, holding, at.stream, &object.hash)?;
            }
        }
    }
    let (state, body, objects) = match &record.record.state {
        State::Is { body, objects } => {
            for object in objects {
                refer(db, at.scope, at.holding, at.stream, &object.hash)?;
            }
            (IS, Some(body.as_slice()), Some(pack(objects)))
        }
        State::Gone => (GONE, None, None),
    };
    db.prepare_cached(
        "INSERT INTO items (stream, place, item, revision, state, kind, schema, body, objects, holding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?
    .execute(params![
        at.stream,
        record.record.order,
        record_item,
        revision as i64,
        state,
        record.kind,
        record.schema,
        body,
        objects,
        at.holding
    ])?;
    Ok(true)
}

fn refer(db: &Connection, scope: i64, holding: i64, stream: i64, hash: &[u8; 32]) -> fastctx_storage::Result<()> {
    db.prepare_cached(
        "INSERT INTO object_refs (scope, holding, stream, hash, count) VALUES (?1, ?2, ?3, ?4, 1)
         ON CONFLICT (scope, holding, stream, hash) DO UPDATE SET count = count + 1",
    )?
    .execute(params![scope, holding, stream, &hash[..]])?;
    Ok(())
}

pub(super) fn unrefer(db: &Connection, scope: i64, holding: i64, stream: i64, hash: &[u8; 32]) -> fastctx_storage::Result<()> {
    db.prepare_cached("UPDATE object_refs SET count = count - 1 WHERE scope = ?1 AND holding = ?2 AND stream = ?3 AND hash = ?4")?
        .execute(params![scope, holding, stream, &hash[..]])?;
    let gone = db
        .prepare_cached("DELETE FROM object_refs WHERE scope = ?1 AND holding = ?2 AND stream = ?3 AND hash = ?4 AND count <= 0")?
        .execute(params![scope, holding, stream, &hash[..]])?;
    if gone > 0 {
        // Collection waits a grace after the last reference went (I2).
        db.prepare_cached("UPDATE objects SET unreferenced = ?2 WHERE hash = ?1 AND state = 1")?
            .execute(params![&hash[..], super::objects::now()])?;
    }
    Ok(())
}

/// Drops the floors of `stream` the current period has caught up with:
/// every record up to `through` is applied, so the item stands as it is.
pub(super) fn floors_passed(db: &Connection, stream: i64, through: u64) -> fastctx_storage::Result<()> {
    db.prepare_cached("DELETE FROM items WHERE stream = ?1 AND state = 2 AND revision <= ?2")?
        .execute(params![stream, through as i64])?;
    Ok(())
}
