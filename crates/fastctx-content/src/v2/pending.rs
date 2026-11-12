//! Records of a kind, or of a schema, a library does not know yet (§2.3):
//! kept in their frames and out of the index, wherever the frame comes in
//! (a hub taking it, a replica importing it, an index rebuilt), until the
//! library knows them and judges them again (`reconsider`).

use super::frame::{Header, KindRecord, open};
use super::judged::Fate;
use super::items::{Applying, apply};
use super::{Authorizer, Library};
use crate::Result;
use crate::error::invalid;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::sync::Arc;

/// Frames whose pending records are judged again in one transaction.
const FRAMES_AT_ONCE: i64 = 64;

/// Keeps the records of `kind` at `schema` in the frame of `stream` at
/// (`epoch`, `first`) whose head is `head` out of the index.
pub(super) fn keep(db: &Connection, stream: i64, kind: u16, schema: u16, epoch: u64, first: u64, head: &[u8; 32]) -> fastctx_storage::Result<()> {
    db.prepare_cached("INSERT OR IGNORE INTO pending_schema (stream, kind, schema, epoch, first, head) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
        .execute(params![stream, kind, schema, epoch as i64, first as i64, &head[..]])?;
    Ok(())
}

/// The kinds and schemas kept out of the index in the frame of `stream` at
/// (`epoch`, `first`) whose head is `head`.
pub(super) fn kept(db: &Connection, stream: i64, epoch: u64, first: u64, head: &[u8; 32]) -> fastctx_storage::Result<Vec<(u16, u16)>> {
    let mut statement =
        db.prepare_cached("SELECT kind, schema FROM pending_schema WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head = ?4")?;
    let rows = statement.query_map(params![stream, epoch as i64, first as i64, &head[..]], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The records of the frame of `stream` at (`epoch`, `first`) whose head is
/// `head` judged and refused, by their place in it: they stay out of the
/// index for good, however it is built again (T03). Another frame put in
/// that place is not refused by it (U04).
pub(super) fn refused(db: &Connection, stream: i64, epoch: u64, first: u64, head: &[u8; 32]) -> fastctx_storage::Result<Vec<u64>> {
    let mut statement =
        db.prepare_cached("SELECT offset FROM refused_records WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head = ?4")?;
    let rows = statement.query_map(params![stream, epoch as i64, first as i64, &head[..]], |row| row.get::<_, i64>(0))?;
    Ok(rows.map(|row| row.map(|offset| offset as u64)).collect::<std::result::Result<_, _>>()?)
}

impl Library {
    /// Judges again the records kept out of the index — of a kind or
    /// schema `kinds` knows now, or waiting behind one of their item (V01)
    /// — a few frames at a time, in the streams' order, each frame's in its
    /// own order (T02): each goes into the index as its frame would have put
    /// it, after `kinds` judges it against its item as the frame's earlier
    /// records and the frames before it left it (U03) — one refused stays
    /// out for good (`refused`); one whose item still has an earlier record
    /// out waits on. Unjudged (`judge` false), they go in as they are. A
    /// frame no longer in its scope's current holding period, or put in its
    /// place since, takes its records with it. How many went in. Call on a
    /// thread that may block.
    pub fn reconsider(&self, kinds: Arc<dyn Authorizer>, judge: bool) -> Result<usize> {
        let hub = self.0.hub;
        let (mut applied, mut after) = (0usize, (-1i64, -1i64, -1i64, Vec::new()));
        // The items with a record still out, by stream and period, as the
        // pass finds them.
        let mut blocked: HashSet<(i64, i64, Vec<u8>)> = HashSet::new();
        // One item history for the whole pass, not one a page (W03).
        let mut kept_history = super::judged::History::default();
        loop {
            let kinds = kinds.clone();
            let from = after.clone();
            let mut passing = std::mem::take(&mut blocked);
            let mut history = std::mem::take(&mut kept_history);
            let (page, last, passing, history) = self.database().write_now_at(Priority::Background, 1 << 16, move |db| {
                let frames: Vec<(i64, i64, i64, Vec<u8>)> = {
                    let mut statement = db.prepare_cached(
                        "SELECT DISTINCT stream, epoch, first, head FROM (
                             SELECT stream, epoch, first, head FROM pending_schema
                             UNION SELECT stream, epoch, first, head FROM deferred_records)
                         WHERE (stream, epoch, first, head) > (?1, ?2, ?3, ?4) ORDER BY stream, epoch, first, head LIMIT ?5",
                    )?;
                    let rows = statement.query_map(params![from.0, from.1, from.2, from.3, FRAMES_AT_ONCE], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })?;
                    rows.collect::<std::result::Result<_, _>>()?
                };
                let last = frames.last().cloned();
                let mut done = 0usize;
                for (stream, epoch, first, head) in frames {
                    let head: [u8; 32] = head.try_into().map_err(|_| invalid("a malformed frame head"))?;
                    let pending = kept(db, stream, epoch as u64, first as u64, &head)?;
                    let known: Vec<(u16, u16)> = pending.iter().copied().filter(|(kind, schema)| kinds.knows(*kind, *schema)).collect();
                    let waiting = super::judged::deferred(db, stream, epoch as u64, first as u64, &head)?;
                    let place: Option<(i64, i64)> = db
                        .prepare_cached("SELECT s.scope, c.holding FROM streams s JOIN scopes c ON c.id = s.scope WHERE s.id = ?1")?
                        .query_row([stream], |row| Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?)))
                        .optional()?
                        .and_then(|(scope, holding)| holding.map(|holding| (scope, holding)));
                    let frame: Option<(Vec<u8>, Vec<u8>)> = match place {
                        Some((_, holding)) => db
                            .prepare_cached("SELECT header, body FROM frames WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
                            .query_row(params![stream, epoch, first, holding], |row| Ok((row.get(0)?, row.get(1)?)))
                            .optional()?,
                        None => None,
                    };
                    let mut gone = true;
                    if let (Some((scope, holding)), Some((header, body))) = (place, frame) {
                        let header = Header::decode(&header).map_err(invalid)?;
                        if header.chain() == head {
                            gone = false;
                            let at = Applying { scope, stream, holding };
                            let records = open(&header, &body).map_err(invalid)?;
                            let refused = refused(db, stream, epoch as u64, first as u64, &head)?;
                            let fate = |offset: usize, record: &KindRecord| {
                                let kind = (record.kind, record.schema);
                                if refused.contains(&(offset as u64)) {
                                    Fate::Out
                                } else if pending.contains(&kind) && !known.contains(&kind) {
                                    Fate::Unknown
                                } else if known.contains(&kind) || waiting.contains(&(offset as u64)) {
                                    Fate::Judge
                                } else {
                                    Fate::Stood
                                }
                            };
                            let went = if judge {
                                let mut pass = super::judged::Blocked::Pass(&mut passing);
                                super::judged::frame(db, at, &header, &records, kinds.as_ref(), &mut history, &mut pass, fate)?
                            } else {
                                let mut went = 0;
                                for (offset, record) in records.iter().enumerate() {
                                    if matches!(fate(offset, record), Fate::Judge) {
                                        apply(db, at, header.first + offset as u64, record)?;
                                        went += 1;
                                    }
                                }
                                db.prepare_cached("DELETE FROM deferred_records WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head = ?4")?
                                    .execute(params![stream, epoch, first, &head[..]])?;
                                went
                            };
                            // In a hub's library, records indexed now are a
                            // change devices ask after, the head as it was
                            // (§28.1): a device holding only a window takes
                            // it again.
                            if hub && went > 0 {
                                super::changes::moved(db, stream, 0, true)?;
                            }
                            // Records from before where readers stood came
                            // in: they read anew.
                            if went > 0 {
                                super::view::rebased(db, stream)?;
                            }
                            done += went;
                        }
                    }
                    let mut forget = db.prepare_cached(
                        "DELETE FROM pending_schema WHERE stream = ?1 AND kind = ?2 AND schema = ?3 AND epoch = ?4 AND first = ?5 AND head = ?6",
                    )?;
                    for (kind, schema) in &known {
                        forget.execute(params![stream, kind, schema, epoch, first, &head[..]])?;
                    }
                    if gone {
                        super::judged::forget_from_frame(db, stream, epoch, first, &head)?;
                    }
                }
                // The pass has gone through every frame with a record out:
                // what it found still out is what waits from now on.
                if last.is_none() {
                    db.prepare_cached("DELETE FROM pending_items")?.execute([])?;
                    let mut keep = db.prepare_cached("INSERT OR IGNORE INTO pending_items (stream, holding, item) VALUES (?1, ?2, ?3)")?;
                    for (stream, holding, item) in &passing {
                        keep.execute(params![stream, holding, item])?;
                    }
                }
                Ok((done, last, passing, history))
            })?;
            blocked = passing;
            kept_history = history;
            applied += page;
            if page > 0 {
                self.wrote();
            }
            match last {
                Some(last) => after = last,
                None => return Ok(applied),
            }
        }
    }
}
