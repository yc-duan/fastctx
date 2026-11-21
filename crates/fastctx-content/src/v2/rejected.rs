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
    /// Which waiting record this is: never another's, even where a later
    /// rejection waits at the same place.
    pub id: i64,
    pub rejected: u64,
    pub position: u64,
    /// The holding period it was rejected in: it is committed again only in
    /// that period.
    pub holding: i64,
    /// Its place in the order the records were first written in.
    pub seq: i64,
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
                    super::items::unrefer(db, at.scope, at.holding, at.stream, &object.hash)?;
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
            // What the frame kept out of the index stays out (§2.3), and so
            // does what was refused once it was known (T03).
            let head = header.chain();
            let kept = super::pending::kept(db, at.stream, header.epoch, header.first, &head)?;
            let mut refused = super::pending::refused(db, at.stream, header.epoch, header.first, &head)?;
            // So does what waits behind an earlier record of its item (V01).
            refused.extend(super::judged::deferred(db, at.stream, header.epoch, header.first, &head)?);
            for (offset, record) in open(&header, &body).map_err(invalid)?.iter().enumerate() {
                if kept.contains(&(record.kind, record.schema)) || refused.contains(&(offset as u64)) {
                    continue;
                }
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
        let kept = self.database().write_now_at(Priority::Normal, 1 << 20, move |db| {
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
            rejected_in(db, scope_id, holding, stream_id, &stream, first)
        })?;
        self.wrote();
        Ok(kept)
    }

    /// The streams of `scope` with records waiting to be committed again in
    /// the current period: each with where its latest rejected run was cut,
    /// and whether the stream has moved to a new epoch since — until it has,
    /// the hub is to open one before they go again.
    pub fn waiting(&self, scope: &str) -> Result<Vec<(String, u64, bool)>> {
        let scope = scope.to_string();
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            let mut statement = db.prepare_cached(
                "SELECT s.name, COALESCE(s.rejected_at, MIN(r.rejected)), s.epoch > COALESCE(s.rejected_epoch, -1)
                 FROM resubmits r JOIN streams s ON s.id = r.stream
                 WHERE s.scope = ?1 AND r.holding = ?2 GROUP BY s.id ORDER BY s.name",
            )?;
            let rows = statement.query_map(params![scope_id, holding], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64, row.get::<_, bool>(2)?))
            })?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Up to `limit` of the records of `stream` waiting to be committed
    /// again, in order.
    pub fn resubmits(&self, scope: &str, stream: &str, limit: usize) -> Result<Vec<Resubmit>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT r.id, r.rejected, r.position, r.holding, r.seq, r.record, r.op FROM resubmits r JOIN streams s ON s.id = r.stream
                 JOIN scopes c ON c.id = s.scope WHERE c.key = ?1 AND s.name = ?2 ORDER BY r.seq, r.id LIMIT ?3",
            )?;
            let rows = statement.query_map(params![scope, stream, limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?;
            let mut resubmits = Vec::new();
            for row in rows {
                let (id, rejected, position, holding, seq, record, op) = row?;
                resubmits.push(Resubmit {
                    id,
                    rejected: rejected as u64,
                    position: position as u64,
                    holding,
                    seq,
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

    /// Opens a new epoch of `stream` at `cut`, where it was cut back to,
    /// under the hub's signed `descriptor`. What was committed to the stream
    /// from `cut` on while the hub was asked goes with what the refusal took
    /// out, in the same transaction: the new epoch begins where the hub
    /// signed it to begin, with nothing of the epoch before left past it.
    pub fn new_epoch_at(&self, scope: &str, stream: &str, epoch: u64, descriptor: Vec<u8>, cut: u64) -> Result<()> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        self.database().write_now_at(Priority::Normal, 1 << 20, move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let found: Option<(i64, i64)> = db
                .prepare_cached("SELECT id, next FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((stream_id, next)) = found else { return Err(invalid("the stream is not here")) };
            if next as u64 > cut {
                rejected_in(db, scope_id, holding, stream_id, &stream, cut)?;
            }
            let changed = db
                .prepare_cached("UPDATE streams SET epoch = ?2 WHERE id = ?1 AND epoch < ?2")?
                .execute(params![stream_id, epoch as i64])?;
            if changed != 1 {
                return Err(invalid("an epoch moves forward"));
            }
            db.prepare_cached("INSERT INTO epochs (stream, epoch, descriptor) VALUES (?1, ?2, ?3)")?
                .execute(params![stream_id, epoch as i64, descriptor])?;
            Ok(())
        })?;
        self.wrote();
        Ok(())
    }

    /// Commits the waiting records of `stream` again, in order, in its
    /// current epoch; each lets go of its place and holds in the same
    /// commit, and an operation points at where its write landed now.
    /// Records no longer writable are left for `refuse`.
    pub fn resubmit(&self, scope: &str, stream: &str, take: usize, signer: Arc<dyn Signer>) -> Result<usize> {
        // A stream this device may hold no more waits for the hub to give
        // it back (`withhold`).
        let (scope_key, stream_name) = (scope.to_string(), stream.to_string());
        let withheld = self.database().read_now(move |db| match super::scopes::held(db, &scope_key)? {
            Some((scope_id, _, Some(holding))) => super::letgo::withheld(db, scope_id, &stream_name, holding),
            _ => Ok(false),
        })?;
        if withheld {
            return Ok(0);
        }
        let waiting = self.resubmits(scope, stream, take)?;
        let Some(holding) = waiting.first().map(|first| first.holding) else { return Ok(0) };
        let batch: Vec<Resubmit> = waiting.into_iter().take_while(|resubmit| resubmit.holding == holding).collect();
        // A record whose item was written again after its run was cut is
        // older than what stands: committed again it would land last and
        // stand instead. It is let go, its holds with it; what is committed
        // is checked to land where the stream stood as that was looked at,
        // nothing written in between (codex-15695bc9 #6).
        let (scope_key, stream_name, looked) = (scope.to_string(), stream.to_string(), batch.clone());
        let (outdated, at) = self.database().write_now_at(Priority::Normal, 1 << 16, move |db| {
            let found: Option<(i64, i64)> = db
                .prepare_cached(
                    "SELECT s.id, s.next FROM streams s JOIN scopes c ON c.id = s.scope WHERE c.key = ?1 AND s.name = ?2",
                )?
                .query_row(params![scope_key, stream_name], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((stream_id, next)) = found else { return Ok((0, 0)) };
            let mut outdated = 0;
            for resubmit in &looked {
                if written_since(db, stream_id, resubmit)? {
                    db.prepare_cached("DELETE FROM resubmits WHERE id = ?1 AND stream = ?2")?
                        .execute(params![resubmit.id, stream_id])?;
                    outdated += 1;
                }
            }
            if outdated > 0 {
                release_holds(db, stream_id)?;
            }
            Ok((outdated, next as u64))
        })?;
        if outdated > 0 {
            self.wrote();
            return Ok(outdated);
        }
        let count = batch.len();
        let raced = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let racing = raced.clone();
        let records: Vec<KindRecord> = batch.iter().map(|resubmit| resubmit.record.clone()).collect();
        let (scope_key, stream_name) = (scope.to_string(), stream.to_string());
        let resubmitted = batch.clone();
        let committed = self.commit_then(
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
            // Judged when first committed; the hub judges each again.
            None,
            move |db, landed| {
                if landed != at {
                    racing.store(true, std::sync::atomic::Ordering::Relaxed);
                    return Err(invalid("the stream was written to as what waited was looked at"));
                }
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
                        .prepare_cached("DELETE FROM resubmits WHERE id = ?1 AND stream = ?2")?
                        .execute(params![resubmit.id, stream_id])?;
                    if taken != 1 {
                        return Err(invalid("a record waiting to be committed again was committed or refused meanwhile"));
                    }
                    if let Some(op) = &resubmit.op {
                        db.prepare_cached("INSERT OR REPLACE INTO op_records (scope, stream, revision, op) VALUES (?1, ?2, ?3, ?4)")?
                            .execute(params![scope_id, stream_name, (landed + index as u64) as i64, op])?;
                    }
                    db.prepare_cached("INSERT OR REPLACE INTO resubmit_order (stream, holding, revision, seq) VALUES (?1, ?2, ?3, ?4)")?
                        .execute(params![stream_id, resubmit.holding, (landed + index as u64) as i64, resubmit.seq])?;
                }
                release_holds(db, stream_id)?;
                Ok(())
            },
        );
        match committed {
            // Looked at again by the caller, which goes on while any goes.
            Err(_) if raced.load(std::sync::atomic::Ordering::Relaxed) => Ok(1),
            Err(error) => Err(error),
            Ok(_) => Ok(count),
        }
    }

    /// Gives up on a waiting record no longer writable: a true source's
    /// write keeps its objects held under its operation, for the user to
    /// drop or send again; a projection's goes, its source still there.
    pub fn refuse(&self, scope: &str, stream: &str, id: i64, why: &str) -> Result<()> {
        let (scope, stream, why) = (scope.to_string(), stream.to_string(), why.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 1024, move |db| {
            let found: Option<(i64, Vec<u8>, Option<String>, i64)> = db
                .prepare_cached(
                    "SELECT r.stream, r.record, r.op, r.seq FROM resubmits r JOIN streams s ON s.id = r.stream JOIN scopes c ON c.id = s.scope
                     WHERE c.key = ?1 AND s.name = ?2 AND r.id = ?3",
                )?
                .query_row(params![scope, stream, id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .optional()?;
            let Some((stream_id, raw, op, seq)) = found else { return Ok(()) };
            let record = decode(&raw).map_err(invalid)?;
            if let Some(op) = &op {
                // The write as the user made it stays in its operation.
                super::ops::refused_in(db, op, seq, &raw, &why)?;
                if let State::Is { objects, .. } = &record.record.state {
                    for object in objects {
                        db.prepare_cached(
                            "INSERT OR IGNORE INTO object_holds (kind, holder, hash, proven) VALUES ('refused', ?1, ?2, 0)",
                        )?
                        .execute(params![op, &object.hash[..]])?;
                    }
                }
            }
            db.prepare_cached("DELETE FROM resubmits WHERE id = ?1")?.execute([id])?;
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
    id: i64,
    stream: i64,
    raw: Vec<u8>,
    op: Option<String>,
    seq: i64,
}

/// When a holding period ends with records of its own still waiting to be
/// committed again, they cannot be: a true source's write becomes a refused
/// one, kept in its operation for the user; a projection's goes. How many.
pub(super) fn end_resubmits(db: &Connection, scope: i64, holding: i64) -> fastctx_storage::Result<usize> {
    let (mut ended, mut streams) = (0, std::collections::BTreeSet::new());
    loop {
        let waiting: Vec<Waiting> = {
            let mut statement = db.prepare_cached(
                "SELECT id, stream, record, op, seq FROM resubmits
                 WHERE holding = ?2 AND stream IN (SELECT id FROM streams WHERE scope = ?1) ORDER BY stream, seq, id LIMIT ?3",
            )?;
            let rows = statement.query_map(params![scope, holding, ROWS_AT_ONCE], |row| {
                Ok(Waiting { id: row.get(0)?, stream: row.get(1)?, raw: row.get(2)?, op: row.get(3)?, seq: row.get(4)? })
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        if waiting.is_empty() {
            break;
        }
        for Waiting { id, stream, raw, op, seq } in waiting {
            if let Some(op) = &op {
                super::ops::refused_in(db, op, seq, &raw, "access to the scope ended before it could be written again")?;
                if let State::Is { objects, .. } = &decode(&raw).map_err(invalid)?.record.state {
                    for object in objects {
                        db.prepare_cached(
                            "INSERT OR IGNORE INTO object_holds (kind, holder, hash, proven) VALUES ('refused', ?1, ?2, 0)",
                        )?
                        .execute(params![op, &object.hash[..]])?;
                    }
                }
            }
            db.prepare_cached("DELETE FROM resubmits WHERE id = ?1")?.execute([id])?;
            streams.insert(stream);
            ended += 1;
        }
    }
    for stream in streams {
        release_holds(db, stream)?;
    }
    // The places records landed at again are the period's alone.
    db.prepare_cached("DELETE FROM resubmit_order WHERE holding = ?2 AND stream IN (SELECT id FROM streams WHERE scope = ?1)")?
        .execute(params![scope, holding])?;
    Ok(ended)
}

/// Lets go of a stream's resubmission holds whose rejected run has no record
/// left waiting.
/// Whether the item of the waiting record `resubmit` was written since its
/// run was cut by a later write than it: a write of its producer's, or a
/// record first written after it committed again before it.
fn written_since(db: &Connection, stream: i64, resubmit: &Resubmit) -> fastctx_storage::Result<bool> {
    let standing: Option<i64> = db
        .prepare_cached("SELECT revision FROM items WHERE stream = ?1 AND item = ?2 ORDER BY revision DESC LIMIT 1")?
        .query_row(params![stream, resubmit.record.record.item], |row| row.get(0))
        .optional()?;
    let Some(revision) = standing.filter(|revision| *revision >= resubmit.rejected as i64) else { return Ok(false) };
    let order: Option<i64> = db
        .prepare_cached("SELECT seq FROM resubmit_order WHERE stream = ?1 AND holding = ?2 AND revision = ?3")?
        .query_row(params![stream, resubmit.holding, revision], |row| row.get(0))
        .optional()?;
    Ok(order.is_none_or(|seq| seq > resubmit.seq))
}

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

/// `Library::rejected` in the transaction `db` is in: the frames of
/// `stream` from `first` out of it, their records kept to commit again. How
/// many records were kept.
pub(super) fn rejected_in(
    db: &Connection,
    scope_id: i64,
    holding: i64,
    stream_id: i64,
    stream: &str,
    first: u64,
) -> fastctx_storage::Result<usize> {
    let key = holder(stream_id, first);
    let epoch: i64 = db.prepare_cached("SELECT epoch FROM streams WHERE id = ?1")?.query_row([stream_id], |row| row.get(0))?;
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
                let revision = header.first + offset as u64;
                let op = super::ops::op_at(db, scope_id, stream, revision)?;
                // A record committed again keeps its place in the
                // order; one written since is placed after all.
                let kept: Option<i64> = db
                    .prepare_cached("SELECT seq FROM resubmit_order WHERE stream = ?1 AND holding = ?2 AND revision = ?3")?
                    .query_row(params![stream_id, holding, revision as i64], |row| row.get(0))
                    .optional()?;
                db.prepare_cached(
                    "INSERT INTO resubmits (stream, rejected, position, holding, epoch, seq, record, op) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?
                .execute(params![stream_id, first as i64, position as i64, holding, epoch, kept.unwrap_or(0), encode(&record), op])?;
                if kept.is_none() {
                    db.prepare_cached("UPDATE resubmits SET seq = id WHERE id = last_insert_rowid()")?.execute([])?;
                }
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
    db.prepare_cached("DELETE FROM resubmit_order WHERE stream = ?1 AND holding = ?2 AND revision >= ?3")?
        .execute(params![stream_id, holding, first as i64])?;
    db.prepare_cached("UPDATE streams SET rejected_at = ?2, rejected_epoch = epoch WHERE id = ?1")?
        .execute(params![stream_id, first as i64])?;
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
    // Rebuilt to before the refused frame: readers read anew (JJ02).
    super::view::rebased(db, stream_id)?;
    Ok(position as usize)
}
