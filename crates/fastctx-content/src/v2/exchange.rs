//! Frames as they go between holders (§5): exported exactly as kept,
//! imported after their signature, chain and holding period are checked,
//! never decoded and re-framed on the way.
//!
//! An import is idempotent by (stream, epoch, first) within the current
//! holding period: the same frame again writes nothing. The same position
//! with other records is refused and kept as evidence. A frame is taken
//! only under its epoch's descriptor from the hub, from that epoch's device
//! and short of any cutoff (§8); a gap fact moves the cursor past a tail
//! that will never come. A frame past what is held answers what is
//! missing. A hub or producer imports only frames whose objects are
//! present (I3); a replica imports them anyway and wants what it lacks.

use super::Library;
use super::frame::{Header, open};
use super::items::{Applying, apply, floors_passed};
use super::objects::{now, present};
use super::scopes::held;
use crate::error::invalid;
use crate::record::State;
use crate::{Error, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;

/// A frame as kept and passed on: its header bytes, signature and
/// compressed records, and the objects its records refer to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFrame {
    pub header: Vec<u8>,
    pub signature: Vec<u8>,
    pub body: Vec<u8>,
    pub objects: Vec<[u8; 32]>,
}

/// What importing a frame did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Imported {
    /// Applied: the period's cursor is now just past it.
    Applied { next: u64, head: [u8; 32] },
    /// Held already in the current period, or before what is held.
    Already,
    /// It starts past what is held: send `from` to `to` first.
    Gap { from: u64, to: u64 },
    /// Its position is held with other records: refused and kept.
    Conflict,
    /// The hub's descriptor of this epoch is not here: take it first.
    Epoch(u64),
}

/// Checks a frame's signature against the key its epoch names.
pub trait Verifier {
    fn verify(&self, header: &Header, header_bytes: &[u8], signature: &[u8]) -> bool;
}

/// Who imports: a hub or the producer requires the objects present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    Hub,
    Replica,
}

fn stream_id(db: &Connection, scope: i64, name: &str) -> fastctx_storage::Result<i64> {
    let found: Option<i64> = db
        .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
        .query_row(params![scope, name], |row| row.get(0))
        .optional()?;
    if let Some(id) = found {
        return Ok(id);
    }
    db.prepare_cached("INSERT INTO streams (scope, name, epoch, next, head, complete_through) VALUES (?1, ?2, 0, 0, NULL, NULL)")?
        .execute(params![scope, name])?;
    Ok(db.last_insert_rowid())
}

impl Library {
    /// Up to `frames` of the current period's frames of `stream`, from the
    /// one holding sequence `from`, as kept.
    pub fn export(&self, scope: &str, stream: &str, from: u64, frames: usize) -> Result<Vec<RawFrame>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let Some((scope, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            let Some(stream) = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope, stream], |row| row.get::<_, i64>(0))
                .optional()?
            else {
                return Ok(Vec::new());
            };
            let mut statement = db.prepare_cached(
                "SELECT epoch, first, header, signature, body FROM frames
                 WHERE stream = ?1 AND holding = ?2 AND first + count > ?3 ORDER BY first LIMIT ?4",
            )?;
            let rows = statement.query_map(params![stream, holding, from as i64, frames as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })?;
            let mut exported = Vec::new();
            for row in rows {
                let (epoch, first, header, signature, body) = row?;
                let mut hashes = db.prepare_cached(
                    "SELECT hash FROM frame_objects WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4 ORDER BY hash",
                )?;
                let objects = hashes
                    .query_map(params![stream, epoch, first, holding], |row| row.get::<_, Vec<u8>>(0))?
                    .map(|hash| hash.map(|hash| hash.try_into().unwrap_or([0; 32])))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                exported.push(RawFrame { header, signature, body, objects });
            }
            Ok(exported)
        })?)
    }

    /// Imports one frame of `scope` as `holder`, into the holding period
    /// `holding` it was asked for in: once that period ended, nothing is
    /// written (§4.5).
    pub fn import(
        &self,
        scope: &str,
        frame: RawFrame,
        verifier: &dyn Verifier,
        holder: Holder,
        holding: i64,
    ) -> Result<Imported> {
        let expected = holding;
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
        let write = move |db: &Connection| -> fastctx_storage::Result<Imported> {
            let Some((scope_id, state, Some(holding))) = held(db, &scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" {
                return Err(invalid("a scope takes frames from elsewhere once it is installed"));
            }
            if holding != expected {
                return Err(invalid("the holding period the frame was asked for in has ended"));
            }
            let stream = stream_id(db, scope_id, &header.stream)?;
            if let Err(epoch) = super::succession::may_import(db, stream, &header)? {
                return Ok(Imported::Epoch(epoch));
            }
            let known: Option<Vec<u8>> = db
                .prepare_cached("SELECT header FROM frames WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
                .query_row(params![stream, header.epoch as i64, header.first as i64, holding], |row| row.get(0))
                .optional()?;
            if let Some(known) = known {
                if known == frame.header {
                    return Ok(Imported::Already);
                }
                db.prepare_cached("INSERT INTO conflicts (stream, epoch, first, why, bytes, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
                    .execute(params![
                        stream,
                        header.epoch as i64,
                        header.first as i64,
                        "the same position with other records",
                        frame.header,
                        now()
                    ])?;
                return Ok(Imported::Conflict);
            }
            let cursor: Option<(i64, Option<Vec<u8>>)> = db
                .prepare_cached("SELECT sequence, head FROM cursors WHERE stream = ?1 AND holding = ?2")?
                .query_row(params![stream, holding], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let (next, previous) = match cursor {
                Some((sequence, head)) => {
                    let head: [u8; 32] = head.unwrap_or_default().try_into().map_err(|_| invalid("a malformed cursor"))?;
                    (sequence as u64 + 1, head)
                }
                None => (0, [0; 32]),
            };
            let (next, previous) = super::succession::skip_gaps(db, stream, holding, (next, previous))?;
            if header.first < next {
                return Ok(Imported::Already);
            }
            if header.first > next {
                return Ok(Imported::Gap { from: next, to: header.first - 1 });
            }
            if header.previous != previous {
                return Err(invalid("the frame does not chain to what is held"));
            }
            let mut hashes = BTreeSet::new();
            for record in &records {
                if let State::Is { objects, .. } = &record.record.state {
                    for object in objects {
                        hashes.insert(object.hash);
                        let here = present(db, &object.hash)?;
                        match (here, holder) {
                            (Some((_, size)), _) if size == object.size => {}
                            (Some(_), _) => return Err(invalid("an object of that hash is here with another length")),
                            (None, Holder::Hub) => return Err(invalid("a record refers to an object that is not here")),
                            (None, Holder::Replica) => {
                                // The scope it is read through, for whoever fetches it.
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
            let head = header.chain();
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
            let at = Applying { scope: scope_id, stream, holding };
            // What was judged of another frame once in this place is not of
            // this one (U04).
            let head = header.chain();
            super::judged::forget_replaced(db, stream, header.epoch, header.first, &head)?;
            // A kind or schema this device does not know stays in its frame,
            // out of the index, as its hub keeps it (§2.3).
            for record in &records {
                if !knows(record.kind, record.schema) {
                    super::pending::keep(db, stream, record.kind, record.schema, header.epoch, header.first, &head)?;
                }
            }
            match &judging {
                // One it knows is judged as the hub judged it, where its
                // frame stands (U02): one the hub let in only once it knew
                // the kind, and refused then, stays out here too.
                Some(kinds) => {
                    let mut history = super::judged::History::of(records.iter().map(|record| record.record.item.clone()));
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
            let end = header.first + u64::from(header.count);
            db.prepare_cached(
                "INSERT INTO cursors (stream, holding, sequence, head) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (stream, holding) DO UPDATE SET sequence = excluded.sequence, head = excluded.head",
            )?
            .execute(params![stream, holding, end as i64 - 1, &head[..]])?;
            db.prepare_cached(
                "UPDATE streams SET epoch = MAX(epoch, ?2), next = MAX(next, ?3), head = ?4 WHERE id = ?1",
            )?
            .execute(params![stream, header.epoch as i64, end as i64, &head[..]])?;
            floors_passed(db, stream, end - 1)?;
            // A gap the hub signed that starts where this frame ends is
            // skipped now, not when some later frame comes (§8).
            let (next, head) = super::succession::skip_gaps(db, stream, holding, (end, head))?;
            Ok(Imported::Applied { next, head })
        };
        // What comes from elsewhere can be had again: it does not wait for
        // the disk (§3).
        Ok(self.database().write_relaxed_now_at(Priority::Normal, bytes, write)?)
    }
}
