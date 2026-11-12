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

/// Pending rows judged again in one transaction.
const ROWS_AT_ONCE: i64 = 256;

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

impl Library {
    /// Judges again the records kept out of the index for a kind or schema
    /// `kinds` knows now, a page at a time, in the order they came: each
    /// goes into the index as the frame it came in had put it, after
    /// `kinds` judges it when `judge` (a hub's library; a replica takes what
    /// its hub judged). One refused stays out. A frame no longer in its
    /// scope's current holding period takes its records with it. How many
    /// went in. Call on a thread that may block.
    pub fn reconsider(&self, kinds: Arc<dyn Authorizer>, judge: bool) -> Result<usize> {
        let (mut applied, mut after) = (0usize, (-1i64, -1i64, -1i64, -1i64, -1i64));
        loop {
            let kinds = kinds.clone();
            let from = after;
            let (page, last) = self.database().write_now_at(Priority::Background, 1 << 16, move |db| {
                let rows: Vec<(i64, i64, i64, i64, i64)> = {
                    let mut statement = db.prepare_cached(
                        "SELECT stream, kind, schema, epoch, first FROM pending_schema
                         WHERE (stream, epoch, first, kind, schema) > (?1, ?2, ?3, ?4, ?5)
                         ORDER BY stream, epoch, first, kind, schema LIMIT ?6",
                    )?;
                    let rows = statement.query_map(params![from.0, from.1, from.2, from.3, from.4, ROWS_AT_ONCE], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                    })?;
                    rows.collect::<std::result::Result<_, _>>()?
                };
                let last = rows.last().map(|&(stream, kind, schema, epoch, first)| (stream, epoch, first, kind, schema));
                let mut done = 0usize;
                for (stream, kind, schema, epoch, first) in rows {
                    let (kind, schema) = (kind as u16, schema as u16);
                    if !kinds.knows(kind, schema) {
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
                        for (offset, record) in open(&header, &body).map_err(invalid)?.iter().enumerate() {
                            if record.kind != kind || record.schema != schema {
                                continue;
                            }
                            let revision = header.first + offset as u64;
                            if judge {
                                let before = super::hub::before(db, stream, holding, &record.record.item)?;
                                if kinds.authorize(&header, record, before.as_ref()).is_err() {
                                    continue;
                                }
                                super::hub::hidden_by_floor(db, at, revision, record)?;
                            }
                            apply(db, at, revision, record)?;
                            done += 1;
                        }
                    }
                    db.prepare_cached("DELETE FROM pending_schema WHERE stream = ?1 AND kind = ?2 AND schema = ?3 AND epoch = ?4 AND first = ?5")?
                        .execute(params![stream, kind, schema, epoch, first])?;
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
