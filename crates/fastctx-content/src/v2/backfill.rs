//! A stream taken whole below its window, in the same holding period (§29
//! 3.3, PH03, P11): a light device switched to holding whole takes the
//! stream's frames from its first up to where its window began, beside the
//! frames that go on from the window's head. They are applied by the
//! revision rule — what a window, a page or a later frame made stays, and
//! so does a floor at a later answer — and chained from the first: once
//! they reach where the window began, the chain is to be the window's head,
//! which the hub's word stood behind. Only then does the stream hold its
//! history whole (`retention` from naught). History shows below the window
//! as it comes; the tail goes on meanwhile.

use super::Library;
use super::exchange::RawFrame;
use super::frame::{Header, open};
use super::items::{Applying, apply};
use super::objects::present;
use super::scopes::held;
use crate::error::invalid;
use crate::record::State;
use crate::{Error, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// What taking a frame below the window did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backfilled {
    /// Taken: the next frame wanted is `at`.
    Applied { at: u64 },
    /// Taken, and the stream's history is whole here now.
    Whole,
    /// Taken before.
    Already,
    /// Frames before it are wanted first, from `from`.
    Gap { from: u64 },
}

/// Where a backfill of a stream stands: the frame it takes next and the
/// sequence its window began at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backfill {
    pub at: u64,
    pub through: u64,
}

/// A backfill as `backfills` keeps it: its holding period, where its window
/// began and the head there, the frame it takes next and the chain there.
type Kept = (i64, i64, Vec<u8>, i64, Vec<u8>);

fn stream_of(db: &Connection, scope: i64, name: &str) -> fastctx_storage::Result<Option<i64>> {
    Ok(db.prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?.query_row(params![scope, name], |row| row.get(0)).optional()?)
}

impl Library {
    /// Begins taking `stream` of `scope` whole below its window, in the
    /// holding period `holding`: where it stands then; none when the stream
    /// holds its history whole already.
    pub fn begin_backfill(&self, scope: &str, stream: &str, holding: i64) -> Result<Option<Backfill>> {
        let (key, name) = (scope.to_string(), stream.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 512, move |db| {
            let Some((scope_id, state, Some(held_in))) = held(db, &key)? else { return Err(invalid("the scope is not held here")) };
            if state != "live" || held_in != holding {
                return Err(invalid("the holding period the backfill was asked for in has ended"));
            }
            let Some(stream) = stream_of(db, scope_id, &name)? else { return Ok(None) };
            let kept: Option<(i64, i64, i64)> = db
                .prepare_cached("SELECT at, through, holding FROM backfills WHERE stream = ?1")?
                .query_row([stream], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .optional()?;
            if let Some((at, through, of)) = kept
                && of == holding
            {
                return Ok(Some(Backfill { at: at as u64, through: through as u64 }));
            }
            let retained: Option<(i64, Option<Vec<u8>>)> = db
                .prepare_cached("SELECT floor, head FROM retention WHERE stream = ?1")?
                .query_row([stream], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((floor, Some(head))) = retained.filter(|(floor, _)| *floor > 0) else { return Ok(None) };
            db.prepare_cached(
                "INSERT OR REPLACE INTO backfills (stream, holding, through, head, at, at_head) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            )?
            .execute(params![stream, holding, floor, head, &[0u8; 32][..]])?;
            Ok(Some(Backfill { at: 0, through: floor as u64 }))
        })?)
    }

    /// Where the backfill of `stream` of `scope` stands; none when none is
    /// under way in its holding period now.
    pub fn backfill(&self, scope: &str, stream: &str) -> Result<Option<Backfill>> {
        let (key, name) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &key)? else { return Ok(None) };
            let Some(stream) = stream_of(db, scope_id, &name)? else { return Ok(None) };
            Ok(db
                .prepare_cached("SELECT at, through FROM backfills WHERE stream = ?1 AND holding = ?2")?
                .query_row(params![stream, holding], |row| {
                    Ok(Backfill { at: row.get::<_, i64>(0)? as u64, through: row.get::<_, i64>(1)? as u64 })
                })
                .optional()?)
        })?)
    }

    /// Takes one frame of `scope` below its stream's window, as asked for in
    /// the holding period `holding`, its signature checked by `verifier`:
    /// applied by the revision rule and chained from the stream's first
    /// frame; the history whole once the chain reaches the window's head.
    pub fn import_backfill(
        &self,
        scope: &str,
        frame: RawFrame,
        verifier: &dyn super::Verifier,
        holding: i64,
    ) -> Result<Backfilled> {
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
        let key = scope.to_string();
        let bytes = frame.body.len() + frame.header.len() + 256 + records.len() * 64;
        let knows = self.knowing();
        let taken = self.database().write_relaxed_now_at(Priority::Normal, bytes, move |db| {
            let Some((scope_id, state, Some(held_in))) = held(db, &key)? else { return Err(invalid("the scope is not held here")) };
            if state != "live" || held_in != holding {
                return Err(invalid("the holding period the frame was asked for in has ended"));
            }
            let Some(stream) = stream_of(db, scope_id, &header.stream)? else { return Err(invalid("no backfill of that stream")) };
            let kept: Option<Kept> = db
                .prepare_cached("SELECT holding, through, head, at, at_head FROM backfills WHERE stream = ?1")?
                .query_row([stream], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
                .optional()?;
            let Some((_, through, head, at, at_head)) = kept.filter(|kept| kept.0 == holding) else {
                return Err(invalid("no backfill of that stream in this holding period"));
            };
            let at_head: [u8; 32] = at_head.try_into().map_err(|_| invalid("a malformed backfill"))?;
            let (at, previous) = super::succession::skip_gaps(db, stream, holding, (at as u64, at_head))?;
            if header.first < at {
                return Ok(Backfilled::Already);
            }
            if header.first > at {
                return Ok(Backfilled::Gap { from: at });
            }
            if header.previous != previous {
                return Err(invalid("the frame does not chain to what was taken below the window"));
            }
            for record in &records {
                if let State::Is { objects, .. } = &record.record.state {
                    for object in objects {
                        match present(db, &object.hash)? {
                            Some((_, size)) if size == object.size => {}
                            Some(_) => return Err(invalid("an object of that hash is here with another length")),
                            None => {
                                db.prepare_cached(
                                    "INSERT OR IGNORE INTO wanted (hash, size, role, priority, hint) VALUES (?1, ?2, 'record', 2, ?3)",
                                )?
                                .execute(params![&object.hash[..], object.size as i64, key])?;
                                db.prepare_cached("INSERT OR IGNORE INTO wanted_in (scope, hash) VALUES (?1, ?2)")?
                                    .execute(params![scope_id, &object.hash[..]])?;
                            }
                        }
                    }
                }
            }
            let chain = header.chain();
            let applying = Applying { scope: scope_id, stream, holding };
            for (offset, record) in records.iter().enumerate() {
                if knows(record.kind, record.schema) {
                    apply(db, applying, header.first + offset as u64, record)?;
                } else {
                    super::pending::keep(db, stream, record.kind, record.schema, header.epoch, header.first, &chain)?;
                }
            }
            let (end, chain) = super::succession::skip_gaps(db, stream, holding, (header.first + u64::from(header.count), chain))?;
            if end < through as u64 {
                db.prepare_cached("UPDATE backfills SET at = ?2, at_head = ?3 WHERE stream = ?1")?
                    .execute(params![stream, end as i64, &chain[..]])?;
                return Ok(Backfilled::Applied { at: end });
            }
            if end != through as u64 || chain[..] != head[..] {
                // Not the history the window stood on: taken again from the
                // first.
                db.prepare_cached("UPDATE backfills SET at = 0, at_head = ?2 WHERE stream = ?1")?
                    .execute(params![stream, &[0u8; 32][..]])?;
                return Err(invalid("the frames below the window do not chain to its head"));
            }
            db.prepare_cached("UPDATE retention SET floor = 0 WHERE stream = ?1")?.execute([stream])?;
            db.prepare_cached("DELETE FROM backfills WHERE stream = ?1")?.execute([stream])?;
            // Held whole: one run covers it, as far as the stream here reached.
            let next: i64 = db
                .prepare_cached("SELECT sequence FROM cursors WHERE stream = ?1 AND holding = ?2")?
                .query_row(params![stream, holding], |row| row.get::<_, i64>(0))
                .optional()?
                .map_or(0, |sequence| sequence + 1);
            let remade: i64 = db
                .prepare_cached("SELECT remade FROM coverage WHERE stream = ?1 AND holding = ?2 ORDER BY through DESC LIMIT 1")?
                .query_row(params![stream, holding], |row| row.get(0))
                .optional()?
                .unwrap_or(0);
            db.prepare_cached("DELETE FROM coverage WHERE stream = ?1")?.execute([stream])?;
            db.prepare_cached(
                "INSERT INTO coverage (stream, holding, remade, low_place, low_item, high_place, high_item, through)
                 VALUES (?1, ?2, ?3, x'', x'', NULL, NULL, ?4)",
            )?
            .execute(params![stream, holding, remade, next])?;
            Ok(Backfilled::Whole)
        })?;
        if !matches!(taken, Backfilled::Already | Backfilled::Gap { .. }) {
            self.wrote();
        }
        Ok(taken)
    }
}
