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
//!
//! A row from a window or an older page goes by when the hub answered it
//! instead (`apply_answered`, §29 3.3, PH01): it is written only where that
//! moment is past the row here, so a page answered earlier never undoes
//! what a later answer or a frame made; and what an answer shows absent is
//! a floor at that moment (`snapshot.rs`).
//!
//! Every row written takes the library's next write number (`writes`), and
//! its stream notes it: readers on this device read by those, whatever
//! order the rows came in (PH04).

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

/// The library's next write number, in the transaction `db` is in.
pub(super) fn next_write(db: &Connection) -> fastctx_storage::Result<i64> {
    Ok(db
        .prepare_cached(
            "INSERT INTO writes (id, last) VALUES (1, 1) ON CONFLICT (id) DO UPDATE SET last = last + 1 RETURNING last",
        )?
        .query_row([], |row| row.get(0))?)
}

/// The library's last write number.
pub(super) fn last_write(db: &Connection) -> fastctx_storage::Result<i64> {
    Ok(db.prepare_cached("SELECT last FROM writes WHERE id = 1")?.query_row([], |row| row.get(0)).optional()?.unwrap_or(0))
}

/// A write number for a row of `stream`, which its stream notes.
pub(super) fn written(db: &Connection, stream: i64) -> fastctx_storage::Result<i64> {
    let local = next_write(db)?;
    db.prepare_cached("UPDATE streams SET written = ?2 WHERE id = ?1")?.execute(params![stream, local])?;
    Ok(local)
}

fn existing(db: &Connection, at: Applying, item: &[u8]) -> fastctx_storage::Result<Option<Row>> {
    Ok(db
        .prepare_cached("SELECT place, revision, state, objects, holding FROM items WHERE stream = ?1 AND item = ?2")?
        .query_row(params![at.stream, item], |row| {
            Ok(Row { place: row.get(0)?, revision: row.get(1)?, state: row.get(2)?, objects: row.get(3)?, holding: row.get(4)? })
        })
        .optional()?)
}

/// Applies `record` at `revision`; whether it changed the index.
pub(super) fn apply(db: &Connection, at: Applying, revision: u64, record: &KindRecord) -> fastctx_storage::Result<bool> {
    let existing = existing(db, at, &record.record.item)?;
    if let Some(Row { revision: known, state, holding, .. }) = &existing {
        let current = *holding == at.holding && *state != FLOOR;
        let replaces = if current { revision as i64 > *known } else { revision as i64 >= *known };
        if !replaces {
            return Ok(false);
        }
    }
    replace(db, at, existing, revision, record)?;
    Ok(true)
}

/// Applies `record`, of `revision`, as the hub answered it at `through` (the
/// sequence its stream was to take next then): only where that is past the
/// row here — a floor at most as late, a row of an earlier revision — so a
/// page answered before what is here undoes nothing; whether it changed the
/// index.
pub(super) fn apply_answered(
    db: &Connection,
    at: Applying,
    revision: u64,
    through: u64,
    record: &KindRecord,
) -> fastctx_storage::Result<bool> {
    let existing = existing(db, at, &record.record.item)?;
    if let Some(Row { revision: known, state, holding, .. }) = &existing {
        let (known, through, revision) = (*known, through as i64, revision as i64);
        let replaces = if *state == FLOOR {
            through >= known
        } else {
            // The same revision of the same period is the row here already.
            known < through && revision >= known && !(revision == known && *holding == at.holding)
        };
        if !replaces {
            return Ok(false);
        }
    }
    replace(db, at, existing, revision, record)?;
    Ok(true)
}

/// Writes `record` at `revision` over the row `existing`, if any.
fn replace(db: &Connection, at: Applying, existing: Option<Row>, revision: u64, record: &KindRecord) -> fastctx_storage::Result<()> {
    let record_item = &record.record.item;
    if let Some(Row { place, objects, holding, .. }) = existing {
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
    let local = written(db, at.stream)?;
    db.prepare_cached(
        "INSERT INTO items (stream, place, item, revision, state, kind, schema, body, objects, holding, local)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
        at.holding,
        local
    ])?;
    Ok(())
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
/// A page asked before they went is asked again (`reclaimed`, PH01).
pub(super) fn floors_passed(db: &Connection, stream: i64, through: u64) -> fastctx_storage::Result<()> {
    let gone = db
        .prepare_cached("DELETE FROM items WHERE stream = ?1 AND state = 2 AND revision <= ?2")?
        .execute(params![stream, through as i64])?;
    if gone > 0 {
        let at = next_write(db)?;
        db.prepare_cached("UPDATE streams SET reclaimed = ?2 WHERE id = ?1")?.execute(params![stream, at])?;
    }
    Ok(())
}
