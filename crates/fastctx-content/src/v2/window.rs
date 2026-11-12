//! A stream's kept frames: dropped by prefix only, for kinds that keep a
//! recent window on devices (§4.6), and cut back after reconciliation
//! (§7.5).
//!
//! A frame goes only when every frame before it went and its caller says
//! all of its records are past the window, so what is kept is always one
//! run of frames, still checked from the chain value at its floor. A
//! producer's own frame goes only once the hub applied it and a checkpoint
//! outside the hub's directory covers that (N09, Q06): if the hub is put
//! back, the producer still has it to send again.

use super::Library;
use super::frame::{Header, open};
use super::items::{Applying, unrefer};
use super::scopes::held;
use crate::error::invalid;
use crate::record::unpack;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};

/// A kept frame: its epoch, first sequence, count, chain value, header and
/// body.
struct Kept {
    epoch: i64,
    first: i64,
    count: i64,
    head: Vec<u8>,
    header: Vec<u8>,
    body: Vec<u8>,
}

/// What pruning a window did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub frames: usize,
    pub items: usize,
    /// Where the stream's kept frames start now.
    pub floor: u64,
}

impl Library {
    /// Drops the frames of `stream` that end at or before `through`, from
    /// its first, stopping at the first that may not go yet; `own` for the
    /// producer's own stream.
    pub fn prune(&self, scope: &str, stream: &str, through: u64, own: bool) -> Result<Pruned> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().write_now_at(Priority::Background, 4096, move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let stream_id: i64 = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get(0))
                .optional()?
                .ok_or_else(|| invalid("no such stream"))?;
            let at = Applying { scope: scope_id, stream: stream_id, holding };
            let mut pruned = Pruned::default();
            let mut floor: Option<(i64, Vec<u8>)> = None;
            // A few frames at a time, from the first: what is kept may be
            // a long history, and only its start goes.
            let mut after = -1i64;
            'walk: loop {
                let frames: Vec<Kept> = {
                    let mut statement = db.prepare_cached(
                        "SELECT epoch, first, count, head, header, body FROM frames
                         WHERE stream = ?1 AND holding = ?2 AND first > ?3 ORDER BY first LIMIT ?4",
                    )?;
                    let rows = statement.query_map(params![stream_id, holding, after, super::rejected::FRAMES_AT_ONCE], |row| {
                        Ok(Kept {
                            epoch: row.get(0)?,
                            first: row.get(1)?,
                            count: row.get(2)?,
                            head: row.get(3)?,
                            header: row.get(4)?,
                            body: row.get(5)?,
                        })
                    })?;
                    rows.collect::<std::result::Result<_, _>>()?
                };
                if frames.is_empty() {
                    break;
                }
                for Kept { epoch, first, count, head, header, body } in frames {
                    if (first + count - 1) as u64 > through {
                        break 'walk;
                    }
                    if own {
                        // Its application is witnessed, and no word on it
                        // from any history waits to be.
                        let witnessed: bool = db
                            .prepare_cached(
                                "SELECT EXISTS (SELECT 1 FROM hub_receipts WHERE stream = ?1 AND epoch = ?2 AND first = ?3
                                                AND kind = 'applied' AND checkpointed = 1)
                                 AND NOT EXISTS (SELECT 1 FROM hub_receipts WHERE stream = ?1 AND epoch = ?2 AND first = ?3
                                                 AND kind IN ('applied', 'parked') AND checkpointed = 0)",
                            )?
                            .query_row(params![stream_id, epoch, first], |row| row.get(0))?;
                        if !witnessed {
                            break 'walk;
                        }
                    }
                    let header = Header::decode(&header).map_err(invalid)?;
                    for record in open(&header, &body).map_err(invalid)? {
                        let row: Option<(Vec<u8>, Option<Vec<u8>>, i64)> = db
                            .prepare_cached("SELECT place, objects, revision FROM items WHERE stream = ?1 AND item = ?2 AND holding = ?3")?
                            .query_row(params![stream_id, record.record.item, holding], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                            .optional()?;
                        // Only an item whose latest revision is in this frame
                        // leaves the index with it.
                        if let Some((place, objects, revision)) = row
                            && revision >= first
                            && revision < first + count
                        {
                            if let Some(objects) = objects {
                                for object in unpack(&objects).map_err(invalid)? {
                                    unrefer(db, at.scope, at.holding, at.stream, &object.hash)?;
                                }
                            }
                            db.prepare_cached("DELETE FROM items WHERE stream = ?1 AND place = ?2 AND item = ?3")?
                                .execute(params![stream_id, place, record.record.item])?;
                            pruned.items += 1;
                        }
                    }
                    for table in ["frame_objects", "frames"] {
                        db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4"))?
                            .execute(params![stream_id, epoch, first, holding])?;
                    }
                    pruned.frames += 1;
                    floor = Some((first + count, head));
                    after = first;
                }
            }
            if let Some((sequence, head)) = floor {
                // What waited in the frames let go goes with them (W01).
                super::judged::rebuild_waiting(db, stream_id)?;
                db.prepare_cached(
                    "INSERT INTO retention (stream, floor, head) VALUES (?1, ?2, ?3)
                     ON CONFLICT (stream) DO UPDATE SET floor = excluded.floor, head = excluded.head",
                )?
                .execute(params![stream_id, sequence, head])?;
                pruned.floor = sequence as u64;
                tidy_wanted(db, scope_id, holding)?;
            } else {
                pruned.floor = db
                    .prepare_cached("SELECT floor FROM retention WHERE stream = ?1")?
                    .query_row([stream_id], |row| row.get::<_, i64>(0))
                    .optional()?
                    .unwrap_or(0) as u64;
            }
            Ok(pruned)
        })?)
    }

    /// Cuts a replica's `stream` back to sequence `through`, the last one
    /// that agrees with the hub — `None` when even its first frame differs —
    /// and rebuilds its index rows from the frames left; the hub's frames
    /// after it come again (§7.5).
    pub fn truncate(&self, scope: &str, stream: &str, through: Option<u64>) -> Result<()> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        self.database().write_now_at(Priority::Normal, 4096, move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let stream_id: i64 = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get(0))
                .optional()?
                .ok_or_else(|| invalid("no such stream"))?;
            truncate_in(db, scope_id, holding, stream_id, through)?;
            // Cut back and indexed again from what is left: readers read anew.
            super::view::rebased(db, stream_id)
        })?;
        self.wrote();
        Ok(())
    }
}

/// The rows of `scope`'s wanted objects that nothing of its holding period
/// `holding` refers to any more, gone: what it no longer wants costs its
/// lookups nothing (`Library::wanted_of`).
pub(super) fn tidy_wanted(db: &fastctx_storage::rusqlite::Connection, scope: i64, holding: i64) -> fastctx_storage::Result<()> {
    db.prepare_cached(
        "DELETE FROM wanted_in WHERE scope = ?1
           AND NOT EXISTS (SELECT 1 FROM object_refs r WHERE r.hash = wanted_in.hash AND r.scope = ?1 AND r.holding = ?2 AND r.count > 0)
           AND NOT EXISTS (SELECT 1 FROM frame_objects f JOIN streams t ON t.id = f.stream
                           WHERE f.hash = wanted_in.hash AND t.scope = ?1 AND f.holding = ?2)",
    )?
    .execute(fastctx_storage::rusqlite::params![scope, holding])?;
    Ok(())
}

/// `Library::truncate` in the transaction `db` is in.
pub(super) fn truncate_in(
    db: &fastctx_storage::rusqlite::Connection,
    scope_id: i64,
    holding: i64,
    stream_id: i64,
    through: Option<u64>,
) -> fastctx_storage::Result<()> {
    // Where a frame ends, or a gap the hub signed that follows it.
    let head: Option<Vec<u8>> = match through {
        None => None,
        Some(through) => Some(
            super::succession::held_end(db, stream_id, holding, through)?
                .ok_or_else(|| invalid("nothing held ends at that sequence"))?
                .to_vec(),
        ),
    };
    let keep = through.map_or(-1, |through| through as i64);
    for table in ["frame_objects", "frames"] {
        db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND holding = ?2 AND first > ?3"))?
            .execute(params![stream_id, holding, keep])?;
    }
    // What was judged of the frames cut away goes with them (U04).
    super::judged::forget_from(db, stream_id, keep + 1)?;
    match &head {
        Some(head) => {
            db.prepare_cached("UPDATE cursors SET sequence = ?3, head = ?4 WHERE stream = ?1 AND holding = ?2")?
                .execute(params![stream_id, holding, keep, head])?;
        }
        // Back to the empty start: the period has no frame of it.
        None => {
            db.prepare_cached("DELETE FROM cursors WHERE stream = ?1 AND holding = ?2")?
                .execute(params![stream_id, holding])?;
        }
    }
    db.prepare_cached("UPDATE streams SET next = ?2, head = ?3 WHERE id = ?1")?
        .execute(params![stream_id, keep + 1, head])?;
    // What is uploaded never reaches past the stream: a record committed at
    // a place cut away is one the hub does not have (AA01).
    db.prepare_cached("UPDATE outbox SET uploaded = MIN(uploaded, ?2) WHERE stream = ?1")?
        .execute(params![stream_id, keep + 1])?;
    super::rejected::rebuild(db, Applying { scope: scope_id, stream: stream_id, holding })?;
    tidy_wanted(db, scope_id, holding)
}
