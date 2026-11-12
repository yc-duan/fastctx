//! Records of a kind, or of a schema, a library does not know yet (§2.3):
//! kept in their frames and out of the index, wherever the frame comes in
//! (a hub taking it, a replica importing it, an index rebuilt), until the
//! library knows them and judges them again (`reconsider`).

use super::frame::{Header, open};
use super::items::{Applying, apply};
use super::{Authorizer, Library};
use crate::Result;
use crate::error::invalid;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::sync::Arc;

/// Frames whose pending records are judged again in one transaction.
const FRAMES_AT_ONCE: i64 = 64;

/// Keeps the records of `kind` at `schema` in the frame of `stream` at
/// (`epoch`, `first`) out of the index.
pub(super) fn keep(db: &Connection, stream: i64, kind: u16, schema: u16, epoch: u64, first: u64) -> fastctx_storage::Result<()> {
    db.prepare_cached("INSERT OR IGNORE INTO pending_schema (stream, kind, schema, epoch, first) VALUES (?1, ?2, ?3, ?4, ?5)")?
        .execute(params![stream, kind, schema, epoch as i64, first as i64])?;
    Ok(())
}

/// The kinds and schemas kept out of the index in the frame of `stream` at
/// (`epoch`, `first`).
pub(super) fn kept(db: &Connection, stream: i64, epoch: u64, first: u64) -> fastctx_storage::Result<Vec<(u16, u16)>> {
    let mut statement = db.prepare_cached("SELECT kind, schema FROM pending_schema WHERE stream = ?1 AND epoch = ?2 AND first = ?3")?;
    let rows = statement.query_map(params![stream, epoch as i64, first as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The records of the frame of `stream` at (`epoch`, `first`) judged and
/// refused once their kind was known, by their place in it: they stay out
/// of the index for good, however it is built again (T03).
pub(super) fn refused(db: &Connection, stream: i64, epoch: u64, first: u64) -> fastctx_storage::Result<Vec<u64>> {
    let mut statement = db.prepare_cached("SELECT offset FROM refused_records WHERE stream = ?1 AND epoch = ?2 AND first = ?3")?;
    let rows = statement.query_map(params![stream, epoch as i64, first as i64], |row| row.get::<_, i64>(0))?;
    Ok(rows.map(|row| row.map(|offset| offset as u64)).collect::<std::result::Result<_, _>>()?)
}

impl Library {
    /// Judges again the records kept out of the index for a kind or schema
    /// `kinds` knows now, a few frames at a time, each frame's in its own
    /// order (T02): each goes into the index as its frame would have put
    /// it, after `kinds` judges it against its item as it stands — one
    /// refused stays out for good (`refused`). A frame no longer in its
    /// scope's current holding period takes its records with it. How many
    /// went in. Call on a thread that may block.
    pub fn reconsider(&self, kinds: Arc<dyn Authorizer>, judge: bool) -> Result<usize> {
        let (mut applied, mut after) = (0usize, (-1i64, -1i64, -1i64));
        loop {
            let kinds = kinds.clone();
            let from = after;
            let (page, last) = self.database().write_now_at(Priority::Background, 1 << 16, move |db| {
                let frames: Vec<(i64, i64, i64)> = {
                    let mut statement = db.prepare_cached(
                        "SELECT DISTINCT stream, epoch, first FROM pending_schema
                         WHERE (stream, epoch, first) > (?1, ?2, ?3) ORDER BY stream, epoch, first LIMIT ?4",
                    )?;
                    let rows = statement.query_map(params![from.0, from.1, from.2, FRAMES_AT_ONCE], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?;
                    rows.collect::<std::result::Result<_, _>>()?
                };
                let last = frames.last().copied();
                let mut done = 0usize;
                for (stream, epoch, first) in frames {
                    let known: Vec<(u16, u16)> = kept(db, stream, epoch as u64, first as u64)?
                        .into_iter()
                        .filter(|(kind, schema)| kinds.knows(*kind, *schema))
                        .collect();
                    if known.is_empty() {
                        continue;
                    }
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
                    if let (Some((scope, holding)), Some((header, body))) = (place, frame) {
                        let header = Header::decode(&header).map_err(invalid)?;
                        let at = Applying { scope, stream, holding };
                        // The frame's records in its own order, those of a
                        // kind still unknown passed over.
                        for (offset, record) in open(&header, &body).map_err(invalid)?.iter().enumerate() {
                            if !known.contains(&(record.kind, record.schema)) {
                                continue;
                            }
                            let revision = header.first + offset as u64;
                            if judge {
                                let before = super::hub::before(db, stream, holding, &record.record.item)?;
                                if kinds.authorize(&header, record, before.as_ref()).is_err() {
                                    db.prepare_cached(
                                        "INSERT OR IGNORE INTO refused_records (stream, epoch, first, offset) VALUES (?1, ?2, ?3, ?4)",
                                    )?
                                    .execute(params![stream, epoch, first, offset as i64])?;
                                    continue;
                                }
                                super::hub::hidden_by_floor(db, at, revision, record)?;
                            }
                            apply(db, at, revision, record)?;
                            done += 1;
                        }
                    }
                    let mut forget = db.prepare_cached(
                        "DELETE FROM pending_schema WHERE stream = ?1 AND kind = ?2 AND schema = ?3 AND epoch = ?4 AND first = ?5",
                    )?;
                    for (kind, schema) in &known {
                        forget.execute(params![stream, kind, schema, epoch, first])?;
                    }
                }
                Ok((done, last))
            })?;
            applied += page;
            match last {
                Some(last) => after = last,
                None => return Ok(applied),
            }
        }
    }
}
