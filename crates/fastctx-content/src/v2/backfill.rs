//! Filling in what a stream's window left below it (design §28.2): from
//! where the index was whole to the window's head H, the frames the hub
//! kept, checked along their own chain from where the filling in reached
//! and at H against the window's head. Their records go into a shadow of
//! the index (`shadow_items`), unread: half way, an item a later frame
//! deletes would show again. Once every frame is in, the shadow goes into
//! the index a batch at a time by the revision rule — what the window or a
//! later frame left there newer stays — and the stream's frames are whole
//! from where they were. A crash goes on from where the filling in reached;
//! the frames, kept as they come, are this device's to give again as any.

use super::Library;
use super::exchange::{RawFrame, Verifier, stream_id};
use super::frame::{Header, KindRecord, open};
use super::items::{Applying, IS, apply, floors_passed};
use super::objects::{now, present};
use super::scopes::held;
use crate::error::invalid;
use crate::record::{Record, State, unpack};
use crate::{Error, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;

/// Items one merge of a backfill takes into the index at most.
pub const MERGED_AT_ONCE: usize = 1_000;

/// A stream being filled in: where it reached, and the window's head it goes
/// to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backfill {
    pub scope: String,
    pub stream: String,
    pub holding: i64,
    pub reached: u64,
    pub to: u64,
}

/// What filling in a frame did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filled {
    /// In; the filling in reached `reached`.
    Applied { reached: u64 },
    /// Every frame to the window's head is in: the shadow is to be merged.
    Ready,
    /// Filled in already.
    Already,
    /// It starts past where the filling in reached: send `from` to `to`.
    Gap { from: u64, to: u64 },
    /// The hub's descriptor of this epoch is not here: take it first.
    Epoch(u64),
}

/// A shadow row as it is taken into the index: identity, order key,
/// revision, state, kind, schema, body and packed objects.
type ShadowRow = (Vec<u8>, Vec<u8>, i64, i64, u16, u16, Option<Vec<u8>>, Option<Vec<u8>>);

/// Where `at` stands past the hub's gap facts that start there, as the
/// cursor's `skip_gaps` moves, without moving the cursor.
fn past_gaps(db: &Connection, stream: i64, mut at: (u64, [u8; 32])) -> fastctx_storage::Result<(u64, [u8; 32])> {
    loop {
        let gap: Option<Vec<u8>> = db
            .prepare_cached("SELECT fact FROM known_gaps WHERE stream = ?1 AND from_sequence = ?2")?
            .query_row(params![stream, at.0 as i64], |row| row.get(0))
            .optional()?;
        let Some(gap) = gap else { return Ok(at) };
        let gap = super::facts::GapFact::decode(&gap).map_err(invalid)?;
        if gap.start != at.1 {
            return Ok(at);
        }
        at = (gap.to + 1, gap.follows);
    }
}

fn array(bytes: Vec<u8>) -> fastctx_storage::Result<[u8; 32]> {
    bytes.try_into().map_err(|_| invalid("a malformed chain value"))
}

impl Library {
    /// Every stream being filled in, in its scope's current holding period.
    pub fn backfills(&self) -> Result<Vec<Backfill>> {
        Ok(self.database().read_now(|db| {
            let mut statement = db.prepare_cached(
                "SELECT s.key, t.name, b.holding, b.reached_next, b.to_next FROM backfills b
                 JOIN streams t ON t.id = b.stream JOIN scopes s ON s.id = t.scope
                 WHERE s.holding = b.holding ORDER BY s.key, t.name",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(Backfill {
                    scope: row.get(0)?,
                    stream: row.get(1)?,
                    holding: row.get(2)?,
                    reached: row.get::<_, i64>(3)? as u64,
                    to: row.get::<_, i64>(4)? as u64,
                })
            })?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Fills in one frame of `scope` below its stream's window, in the
    /// holding period `holding` it was asked for in: checked as an imported
    /// frame is, chained from where the filling in reached.
    pub fn backfill(&self, scope: &str, frame: RawFrame, verifier: &dyn Verifier, holding: i64) -> Result<Filled> {
        let header = Header::decode(&frame.header).map_err(Error::Invalid)?;
        if header.scope != scope {
            return Err(Error::Invalid("the frame is of another scope".into()));
        }
        if !verifier.verify(&header, &frame.header, &frame.signature) {
            return Err(Error::Invalid("the frame's signature does not verify".into()));
        }
        let records = open(&header, &frame.body).map_err(Error::Invalid)?;
        for record in &records {
            record.record.check().map_err(Error::Invalid)?;
        }
        let scope = scope.to_string();
        let bytes = frame.body.len() + frame.header.len() + 256 + records.len() * 64;
        let knows = self.knowing();
        let judging = self.judging();
        let write = move |db: &Connection| -> fastctx_storage::Result<Filled> {
            let Some((scope_id, state, Some(held_in))) = held(db, &scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" || held_in != holding {
                return Err(invalid("the holding period the frame was asked for in has ended"));
            }
            let stream = stream_id(db, scope_id, &header.stream)?;
            let Some((to, to_head, reached, reached_head)): Option<(i64, Vec<u8>, i64, Vec<u8>)> = db
                .prepare_cached("SELECT to_next, to_head, reached_next, reached_head FROM backfills WHERE stream = ?1 AND holding = ?2")?
                .query_row(params![stream, holding], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .optional()?
            else {
                return Err(invalid("nothing of the stream is being filled in"));
            };
            let (to, to_head) = (to as u64, array(to_head)?);
            let (reached, previous) = past_gaps(db, stream, (reached as u64, array(reached_head)?))?;
            if reached >= to {
                return Ok(Filled::Ready);
            }
            if let Err(epoch) = super::succession::may_import(db, stream, &header)? {
                return Ok(Filled::Epoch(epoch));
            }
            if header.first < reached {
                return Ok(Filled::Already);
            }
            if header.first > reached {
                return Ok(Filled::Gap { from: reached, to: header.first - 1 });
            }
            if header.previous != previous {
                return Err(invalid("the frame does not chain to what is filled in"));
            }
            let head = header.chain();
            let end = header.first + u64::from(header.count);
            if end > to {
                return Err(invalid("the frame runs past the window's head"));
            }
            let known: Option<Vec<u8>> = db
                .prepare_cached("SELECT header FROM frames WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
                .query_row(params![stream, header.epoch as i64, header.first as i64, holding], |row| row.get(0))
                .optional()?;
            match known {
                Some(known) if known != frame.header => {
                    db.prepare_cached("INSERT INTO conflicts (stream, epoch, first, why, bytes, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
                        .execute(params![
                            stream,
                            header.epoch as i64,
                            header.first as i64,
                            "the same position with other records, filling in",
                            frame.header,
                            now()
                        ])?;
                    return Err(invalid("the frame's position is held with other records"));
                }
                Some(_) => {}
                None => {
                    let mut hashes = BTreeSet::new();
                    for record in &records {
                        if let State::Is { objects, .. } = &record.record.state {
                            for object in objects {
                                hashes.insert(object.hash);
                                match present(db, &object.hash)? {
                                    Some((_, size)) if size == object.size => {}
                                    Some(_) => return Err(invalid("an object of that hash is here with another length")),
                                    None => {
                                        db.prepare_cached(
                                            "INSERT OR IGNORE INTO wanted (hash, size, role, priority, hint) VALUES (?1, ?2, 'record', 1, ?3)",
                                        )?
                                        .execute(params![&object.hash[..], object.size as i64, scope])?;
                                        db.prepare_cached("INSERT OR IGNORE INTO wanted_in (scope, hash) VALUES (?1, ?2)")?
                                            .execute(params![scope_id, &object.hash[..]])?;
                                    }
                                }
                            }
                        }
                    }
                    db.prepare_cached(
                        "INSERT INTO frames (stream, epoch, first, holding, count, head, header, signature, body, change)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
                    )?
                    .execute(params![
                        stream,
                        header.epoch as i64,
                        header.first as i64,
                        holding,
                        header.count,
                        &head[..],
                        frame.header,
                        frame.signature,
                        frame.body
                    ])?;
                    let mut referring = db.prepare_cached(
                        "INSERT OR IGNORE INTO frame_objects (stream, epoch, first, holding, hash) VALUES (?1, ?2, ?3, ?4, ?5)",
                    )?;
                    for hash in &hashes {
                        referring.execute(params![stream, header.epoch as i64, header.first as i64, holding, &hash[..]])?;
                    }
                }
            }
            super::judged::forget_replaced(db, stream, header.epoch, header.first, &head)?;
            for record in &records {
                if !knows(record.kind, record.schema) {
                    super::pending::keep(db, stream, record.kind, record.schema, header.epoch, header.first, &head)?;
                }
            }
            let at = Applying { scope: scope_id, stream, holding, shadow: true };
            match &judging {
                Some(kinds) => {
                    let mut history = super::judged::History::of(
                        records.iter().enumerate().map(|(offset, record)| (record.record.item.clone(), header.first + offset as u64)),
                    );
                    super::judged::frame(db, at, &header, &records, kinds.as_ref(), &mut history, &mut super::judged::Blocked::Stored, |_, record| {
                        if knows(record.kind, record.schema) { super::judged::Fate::Judge } else { super::judged::Fate::Unknown }
                    })?;
                }
                None => {
                    for (offset, record) in records.iter().enumerate() {
                        apply(db, at, header.first + offset as u64, record)?;
                    }
                }
            }
            let (reached, head) = past_gaps(db, stream, (end, head))?;
            if reached >= to && head != to_head {
                return Err(invalid("what was filled in does not end at the window's head"));
            }
            db.prepare_cached("UPDATE backfills SET reached_next = ?3, reached_head = ?4 WHERE stream = ?1 AND holding = ?2")?
                .execute(params![stream, holding, reached as i64, &head[..]])?;
            Ok(if reached >= to { Filled::Ready } else { Filled::Applied { reached } })
        };
        Ok(self.database().write_relaxed_now_at(Priority::Background, bytes, write)?)
    }

    /// Takes up to `limit` items of the filled-in shadow of `stream` into
    /// the index by the revision rule, once every frame to the window's head
    /// is in; whether the stream is whole now, nothing being filled in.
    pub fn merge_backfill(&self, scope: &str, stream: &str, holding: i64, limit: usize) -> Result<bool> {
        let (key, name) = (scope.to_string(), stream.to_string());
        Ok(self.database().write_now_at(Priority::Background, limit.max(1) * 512, move |db| {
            let Some((scope_id, state, Some(held_in))) = held(db, &key)? else { return Ok(true) };
            if state != "live" || held_in != holding {
                return Err(invalid("the holding period the backfill was in has ended"));
            }
            let Some(stream): Option<i64> = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, name], |row| row.get(0))
                .optional()?
            else {
                return Ok(true);
            };
            let Some((to, reached, from, from_head)): Option<(i64, i64, i64, Vec<u8>)> = db
                .prepare_cached("SELECT to_next, reached_next, from_next, from_head FROM backfills WHERE stream = ?1 AND holding = ?2")?
                .query_row(params![stream, holding], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .optional()?
            else {
                return Ok(true);
            };
            if reached < to {
                return Ok(false);
            }
            let rows: Vec<ShadowRow> = {
                let mut statement = db.prepare_cached(
                    "SELECT item, place, revision, state, kind, schema, body, objects FROM shadow_items
                     WHERE stream = ?1 AND holding = ?2 ORDER BY item LIMIT ?3",
                )?;
                let rows = statement.query_map(params![stream, holding, limit.max(1) as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
                })?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let at = Applying { scope: scope_id, stream, holding, shadow: false };
            for (item, place, revision, state, kind, schema, body, objects) in &rows {
                let state = if *state == IS {
                    State::Is { body: body.clone().unwrap_or_default(), objects: unpack(objects.as_deref().unwrap_or_default()).map_err(invalid)? }
                } else {
                    State::Gone
                };
                let record = KindRecord { kind: *kind, schema: *schema, record: Record { item: item.clone(), order: place.clone(), state } };
                apply(db, at, *revision as u64, &record)?;
                db.prepare_cached("DELETE FROM shadow_items WHERE stream = ?1 AND holding = ?2 AND item = ?3")?
                    .execute(params![stream, holding, item])?;
            }
            if rows.len() >= limit.max(1) {
                return Ok(false);
            }
            // Whole: nothing is filled in any more, and the stream's frames
            // here begin where they began before the window.
            db.prepare_cached("DELETE FROM backfills WHERE stream = ?1 AND holding = ?2")?.execute(params![stream, holding])?;
            if from == 0 {
                db.prepare_cached("DELETE FROM retention WHERE stream = ?1")?.execute([stream])?;
            } else {
                db.prepare_cached("UPDATE retention SET floor = ?2, head = ?3 WHERE stream = ?1")?.execute(params![stream, from, from_head])?;
            }
            floors_passed(db, stream, to as u64 - 1)?;
            Ok(true)
        })?)
    }
}
