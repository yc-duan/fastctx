//! A producer's side of a rejected frame (§6.5, P07, Q03).
//!
//! When the hub rejects a frame, the frame and every later frame of its
//! epoch leave the canonical stream. In one transaction the producer copies
//! their records into `resubmits`, holds every object they refer to in
//! `object_holds` — the truncation removes those frames' references, and an
//! attachment not yet uploaded lives only here — truncates its stream to
//! before the rejected frame and rebuilds the stream's index rows from the
//! frames that remain. Records still writable are committed again under a
//! new epoch, and each resubmission lets go of its holds in its own commit;
//! a refused true source's write keeps its objects under its operation
//! until the user drops it or sends it again.

use super::Library;
use super::frame::{Header, KindRecord, Signer, open};
use super::items::{Applying, apply};
use super::scopes::held;
use crate::error::invalid;
use crate::record::State;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::sync::Arc;

/// One record waiting to be committed again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resubmit {
    pub rejected: u64,
    pub position: u64,
    /// The holding period it was rejected in: it is committed again only in
    /// that period.
    pub holding: i64,
    pub record: KindRecord,
    pub op: Option<String>,
}

fn encode(record: &KindRecord) -> Vec<u8> {
    let mut raw = Vec::new();
    raw.extend_from_slice(&record.kind.to_le_bytes());
    raw.extend_from_slice(&record.schema.to_le_bytes());
    record.record.encode(&mut raw);
    raw
}

pub(super) fn decode(raw: &[u8]) -> std::result::Result<KindRecord, String> {
    if raw.len() < 4 {
        return Err("a kept record is cut short".into());
    }
    let mut rest = &raw[4..];
    Ok(KindRecord {
        kind: u16::from_le_bytes([raw[0], raw[1]]),
        schema: u16::from_le_bytes([raw[2], raw[3]]),
        record: crate::Record::decode(&mut rest)?,
    })
}

/// Frames read at once when a stream is walked.
pub(super) const FRAMES_AT_ONCE: i64 = 16;

/// Index rows, or waiting records, handled at once: a stream's index or a
/// period's waiting records are not held whole either.
const ROWS_AT_ONCE: i64 = 64;

fn holder(stream: i64, rejected: u64) -> String {
    format!("{stream}:{rejected}")
}

/// Rebuilds the index rows of `stream` in the current period from its
/// frames there, in order (§7.5).
pub(super) fn rebuild(db: &Connection, at: Applying) -> fastctx_storage::Result<()> {
    /// An index row going: where it stands, its state and object references.
    struct Going {
        place: Vec<u8>,
        item: Vec<u8>,
        state: i64,
        objects: Option<Vec<u8>>,
    }
    loop {
        let rows: Vec<Going> = {
            let mut statement =
                db.prepare_cached("SELECT place, item, state, objects FROM items WHERE stream = ?1 AND holding = ?2 LIMIT ?3")?;
            let rows = statement.query_map(params![at.stream, at.holding, ROWS_AT_ONCE], |row| {
                Ok(Going { place: row.get(0)?, item: row.get(1)?, state: row.get(2)?, objects: row.get(3)? })
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        if rows.is_empty() {
            break;
        }
        for Going { place, item, state, objects } in rows {
            if state == super::items::IS
                && let Some(objects) = objects
            {
                for object in crate::record::unpack(&objects).map_err(invalid)? {
                    super::items::unrefer(db, at.scope, at.holding, &object.hash)?;
                }
            }
            db.prepare_cached("DELETE FROM items WHERE stream = ?1 AND place = ?2 AND item = ?3")?
                .execute(params![at.stream, place, item])?;
        }
    }
    // A few frames at a time: a stream's whole history is not held at once.
    let mut after = -1i64;
    loop {
        let frames: Vec<(i64, Vec<u8>, Vec<u8>)> = {
            let mut statement = db.prepare_cached(
                "SELECT first, header, body FROM frames WHERE stream = ?1 AND holding = ?2 AND first > ?3 ORDER BY first LIMIT ?4",
            )?;
            let rows = statement
                .query_map(params![at.stream, at.holding, after, FRAMES_AT_ONCE], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        if frames.is_empty() {
            return Ok(());
        }
        for (first, header, body) in frames {
            let header = Header::decode(&header).map_err(invalid)?;
            for (offset, record) in open(&header, &body).map_err(invalid)?.iter().enumerate() {
                apply(db, at, header.first + offset as u64, record)?;
            }
            after = first;
        }
    }
}

impl Library {
    /// Takes the frame of `stream` at `first`, rejected by the hub, and every
    /// later one out of the stream, keeping their records to commit again.
    pub fn rejected(&self, scope: &str, stream: &str, first: u64) -> Result<usize> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 1 << 20, move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            let Some(stream_id) = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get::<_, i64>(0))
                .optional()?
            else {
                return Ok(0);
            };
            let key = holder(stream_id, first);
            let mut position = 0u64;
            // A few frames at a time: the run after a rejected frame can be
            // long when its producer was away.
            let mut after = first as i64 - 1;
            loop {
                let frames: Vec<(i64, Vec<u8>, Vec<u8>)> = {
                    let mut statement = db.prepare_cached(
                        "SELECT first, header, body FROM frames WHERE stream = ?1 AND holding = ?2 AND first > ?3 ORDER BY first LIMIT ?4",
                    )?;
                    let rows = statement.query_map(params![stream_id, holding, after, FRAMES_AT_ONCE], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?;
                    rows.collect::<std::result::Result<_, _>>()?
                };
                if frames.is_empty() {
                    break;
                }
                for (start, header, body) in frames {
                    let header = Header::decode(&header).map_err(invalid)?;
                    for (offset, record) in open(&header, &body).map_err(invalid)?.into_iter().enumerate() {
                        let op = super::ops::op_at(db, scope_id, &stream, header.first + offset as u64)?;
                        db.prepare_cached(
                            "INSERT INTO resubmits (stream, rejected, position, holding, record, op) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        )?
                        .execute(params![stream_id, first as i64, position as i64, holding, encode(&record), op])?;
                        if let State::Is { objects, .. } = &record.record.state {
                            for object in objects {
                                db.prepare_cached(
                                    "INSERT OR IGNORE INTO object_holds (kind, holder, hash, proven) VALUES ('resubmit', ?1, ?2, 0)",
                                )?
                                .execute(params![key, &object.hash[..]])?;
                            }
                        }
                        position += 1;
                    }
                    after = start;
                }
            }
            for table in ["frame_objects", "frames"] {
                db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND holding = ?2 AND first >= ?3"))?
                    .execute(params![stream_id, holding, first as i64])?;
            }
            // The waiting records carry their operations; the places they
            // stood at are someone else's once the stream goes on.
            db.prepare_cached("DELETE FROM op_records WHERE scope = ?1 AND stream = ?2 AND revision >= ?3")?
                .execute(params![scope_id, stream, first as i64])?;
            let before: Option<Vec<u8>> = db
                .prepare_cached("SELECT head FROM frames WHERE stream = ?1 AND holding = ?2 ORDER BY first DESC LIMIT 1")?
                .query_row(params![stream_id, holding], |row| row.get(0))
                .optional()?;
            db.prepare_cached("UPDATE streams SET next = ?2, head = ?3 WHERE id = ?1")?
                .execute(params![stream_id, first as i64, before])?;
            db.prepare_cached("UPDATE cursors SET sequence = ?3, head = ?4 WHERE stream = ?1 AND holding = ?2")?
                .execute(params![stream_id, holding, first as i64 - 1, before])?;
            db.prepare_cached("UPDATE outbox SET uploaded = MIN(uploaded, ?2) WHERE stream = ?1")?
                .execute(params![stream_id, first as i64])?;
            rebuild(db, Applying { scope: scope_id, stream: stream_id, holding })?;
            Ok(position as usize)
        })?)
    }

    /// Up to `limit` of the records of `stream` waiting to be committed
    /// again, in order.
    pub fn resubmits(&self, scope: &str, stream: &str, limit: usize) -> Result<Vec<Resubmit>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT r.rejected, r.position, r.holding, r.record, r.op FROM resubmits r JOIN streams s ON s.id = r.stream
                 JOIN scopes c ON c.id = s.scope WHERE c.key = ?1 AND s.name = ?2 ORDER BY r.rejected, r.position LIMIT ?3",
            )?;
            let rows = statement.query_map(params![scope, stream, limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?;
            let mut resubmits = Vec::new();
            for row in rows {
                let (rejected, position, holding, record, op) = row?;
                resubmits.push(Resubmit {
                    rejected: rejected as u64,
                    position: position as u64,
                    holding,
                    record: decode(&record).map_err(invalid)?,
                    op,
                });
            }
            Ok(resubmits)
        })?)
    }

    /// Opens a new epoch of `stream` from where it stands, under the hub's
    /// signed `descriptor`, before what was rejected is committed again.
    pub fn new_epoch(&self, scope: &str, stream: &str, epoch: u64, descriptor: Vec<u8>) -> Result<()> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 256 + descriptor.len(), move |db| {
            let Some((scope_id, _, Some(_))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let changed = db
                .prepare_cached("UPDATE streams SET epoch = ?3 WHERE scope = ?1 AND name = ?2 AND epoch < ?3")?
                .execute(params![scope_id, stream, epoch as i64])?;
            if changed != 1 {
                return Err(invalid("an epoch moves forward"));
            }
            db.prepare_cached(
                "INSERT INTO epochs (stream, epoch, descriptor) VALUES ((SELECT id FROM streams WHERE scope = ?1 AND name = ?2), ?3, ?4)",
            )?
            .execute(params![scope_id, stream, epoch as i64, descriptor])?;
            Ok(())
        })?)
    }

    /// Commits the waiting records of `stream` again, in order, in its
    /// current epoch; each lets go of its place and holds in the same
    /// commit, and an operation points at where its write landed now.
    /// Records no longer writable are left for `refuse`.
    pub fn resubmit(&self, scope: &str, stream: &str, take: usize, signer: Arc<dyn Signer>) -> Result<usize> {
        let waiting = self.resubmits(scope, stream, take)?;
        let Some(holding) = waiting.first().map(|first| first.holding) else { return Ok(0) };
        let batch: Vec<Resubmit> = waiting.into_iter().take_while(|resubmit| resubmit.holding == holding).collect();
        let count = batch.len();
        let records: Vec<KindRecord> = batch.iter().map(|resubmit| resubmit.record.clone()).collect();
        let (scope_key, stream_name) = (scope.to_string(), stream.to_string());
        let resubmitted = batch.clone();
        self.commit_then(
            super::Commit {
                scope: scope.into(),
                stream: stream.into(),
                records,
                leases: Vec::new(),
                durable: true,
                op: None,
                holding,
            },
            signer,
            move |db, landed| {
                let (scope_id, stream_id): (i64, i64) = db
                    .prepare_cached(
                        "SELECT c.id, s.id FROM streams s JOIN scopes c ON c.id = s.scope WHERE c.key = ?1 AND s.name = ?2",
                    )?
                    .query_row(params![scope_key, stream_name], |row| Ok((row.get(0)?, row.get(1)?)))?;
                // Each operation now points at where its records landed. A
                // record no longer waiting was committed or refused meanwhile:
                // nothing of this commit stands.
                for (index, resubmit) in resubmitted.iter().enumerate() {
                    let taken = db
                        .prepare_cached("DELETE FROM resubmits WHERE stream = ?1 AND rejected = ?2 AND position = ?3")?
                        .execute(params![stream_id, resubmit.rejected as i64, resubmit.position as i64])?;
                    if taken != 1 {
                        return Err(invalid("a record waiting to be committed again was committed or refused meanwhile"));
                    }
                    if let Some(op) = &resubmit.op {
                        db.prepare_cached("INSERT OR REPLACE INTO op_records (scope, stream, revision, op) VALUES (?1, ?2, ?3, ?4)")?
                            .execute(params![scope_id, stream_name, (landed + index as u64) as i64, op])?;
                    }
                }
                release_holds(db, stream_id)?;
                Ok(())
            },
        )?;
        Ok(count)
    }

    /// Gives up on a waiting record no longer writable: a true source's
    /// write keeps its objects held under its operation, for the user to
    /// drop or send again; a projection's goes, its source still there.
    pub fn refuse(&self, scope: &str, stream: &str, rejected: u64, position: u64, why: &str) -> Result<()> {
        let (scope, stream, why) = (scope.to_string(), stream.to_string(), why.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 1024, move |db| {
            let found: Option<(i64, Vec<u8>, Option<String>)> = db
                .prepare_cached(
                    "SELECT r.stream, r.record, r.op FROM resubmits r JOIN streams s ON s.id = r.stream JOIN scopes c ON c.id = s.scope
                     WHERE c.key = ?1 AND s.name = ?2 AND r.rejected = ?3 AND r.position = ?4",
                )?
                .query_row(params![scope, stream, rejected as i64, position as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()?;
            let Some((stream_id, raw, op)) = found else { return Ok(()) };
            let record = decode(&raw).map_err(invalid)?;
            if let Some(op) = &op {
                // The write as the user made it stays in its operation.
                super::ops::refused_in(db, op, &raw, &why)?;
                if let State::Is { objects, .. } = &record.record.state {
                    for object in objects {
                        db.prepare_cached(
                            "INSERT OR IGNORE INTO object_holds (kind, holder, hash, proven) VALUES ('refused', ?1, ?2, 0)",
                        )?
                        .execute(params![op, &object.hash[..]])?;
                    }
                }
            }
            db.prepare_cached("DELETE FROM resubmits WHERE stream = ?1 AND rejected = ?2 AND position = ?3")?
                .execute(params![stream_id, rejected as i64, position as i64])?;
            release_holds(db, stream_id)?;
            Ok(())
        })?)
    }

    /// The user dropped a write that could not be written, or sent it again
    /// as a new one: its objects are held no longer.
    pub fn dropped(&self, op: &str) -> Result<()> {
        let op = op.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 128, move |db| {
            let hashes: Vec<Vec<u8>> = {
                let mut statement = db.prepare_cached("SELECT hash FROM object_holds WHERE kind = 'refused' AND holder = ?1")?;
                let rows = statement.query_map([&op], |row| row.get(0))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            db.prepare_cached("DELETE FROM object_holds WHERE kind = 'refused' AND holder = ?1")?.execute([&op])?;
            db.prepare_cached("UPDATE ops SET state = 'dropped', kept = NULL WHERE op = ?1 AND state = 'refused'")?
                .execute([&op])?;
            for hash in hashes {
                db.prepare_cached("UPDATE objects SET unreferenced = ?2 WHERE hash = ?1 AND state = 1")?
                    .execute(params![hash, super::objects::now()])?;
            }
            Ok(())
        })?)
    }
}

/// A record waiting to be committed again, as kept.
struct Waiting {
    stream: i64,
    rejected: i64,
    position: i64,
    raw: Vec<u8>,
    op: Option<String>,
}

/// When a holding period ends with records of its own still waiting to be
/// committed again, they cannot be: a true source's write becomes a refused
/// one, kept in its operation for the user; a projection's goes. How many.
pub(super) fn end_resubmits(db: &Connection, scope: i64, holding: i64) -> fastctx_storage::Result<usize> {
    let (mut ended, mut streams) = (0, std::collections::BTreeSet::new());
    loop {
        let waiting: Vec<Waiting> = {
            let mut statement = db.prepare_cached(
                "SELECT stream, rejected, position, record, op FROM resubmits
                 WHERE holding = ?2 AND stream IN (SELECT id FROM streams WHERE scope = ?1) LIMIT ?3",
            )?;
            let rows = statement.query_map(params![scope, holding, ROWS_AT_ONCE], |row| {
                Ok(Waiting { stream: row.get(0)?, rejected: row.get(1)?, position: row.get(2)?, raw: row.get(3)?, op: row.get(4)? })
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        if waiting.is_empty() {
            break;
        }
        for Waiting { stream, rejected, position, raw, op } in waiting {
            if let Some(op) = &op {
                super::ops::refused_in(db, op, &raw, "access to the scope ended before it could be written again")?;
                if let State::Is { objects, .. } = &decode(&raw).map_err(invalid)?.record.state {
                    for object in objects {
                        db.prepare_cached(
                            "INSERT OR IGNORE INTO object_holds (kind, holder, hash, proven) VALUES ('refused', ?1, ?2, 0)",
                        )?
                        .execute(params![op, &object.hash[..]])?;
                    }
                }
            }
            db.prepare_cached("DELETE FROM resubmits WHERE stream = ?1 AND rejected = ?2 AND position = ?3")?
                .execute(params![stream, rejected, position])?;
            streams.insert(stream);
            ended += 1;
        }
    }
    for stream in streams {
        release_holds(db, stream)?;
    }
    Ok(ended)
}

/// Lets go of a stream's resubmission holds whose rejected run has no record
/// left waiting.
fn release_holds(db: &Connection, stream: i64) -> fastctx_storage::Result<()> {
    let prefix = format!("{stream}:");
    let runs: Vec<String> = {
        let mut statement =
            db.prepare_cached("SELECT DISTINCT holder FROM object_holds WHERE kind = 'resubmit' AND holder LIKE ?1 || '%'")?;
        let rows = statement.query_map([&prefix], |row| row.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for run in runs {
        let Some(rejected) = run.strip_prefix(&prefix).and_then(|rest| rest.parse::<i64>().ok()) else { continue };
        let waiting: i64 = db
            .prepare_cached("SELECT COUNT(*) FROM resubmits WHERE stream = ?1 AND rejected = ?2")?
            .query_row(params![stream, rejected], |row| row.get(0))?;
        if waiting == 0 {
            db.prepare_cached(
                "UPDATE objects SET unreferenced = ?2 WHERE state = 1 AND hash IN
                 (SELECT hash FROM object_holds WHERE kind = 'resubmit' AND holder = ?1)",
            )?
            .execute(params![run, super::objects::now()])?;
            db.prepare_cached("DELETE FROM object_holds WHERE kind = 'resubmit' AND holder = ?1")?.execute([&run])?;
        }
    }
    Ok(())
}
