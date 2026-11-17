//! A replica's frames of a stream past where the hub holds it now, set
//! aside rather than lost (fff AT02, AT03, AU01, AU02, AU08). After the hub
//! was put back from an older copy, the streams it orders it writes again
//! from its records, which may no longer hold all it once signed; this
//! machine's frames from the first that reaches past the hub's place on are
//! kept as evidence of their holding period — whole, their objects held,
//! gone with the period when it is let go — and the stream is cut back
//! before that frame: at the end of one of its own frames, or to its start.

use super::Library;
use super::exchange::RawFrame;
use super::frame::{Header, open};
use super::scopes::held;
use super::window::truncate_in;
use crate::Result;
use crate::State;
use crate::error::invalid;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};
use std::collections::BTreeSet;

/// How many frames are read at a time: a long tail costs its frames' size
/// a batch at a time, never all at once.
const AT_ONCE: i64 = 64;

/// A frame of a stream set aside: its first sequence, its header,
/// signature and body.
type Aside = (i64, Vec<u8>, Vec<u8>, Vec<u8>);

impl Library {
    /// Sets aside the frames of `stream` of `scope`, in the holding period
    /// it is in, from the first that holds `from` or anything after it, and
    /// cuts the stream back before that one, saying `why`. Answers where the
    /// stream goes on from: its next sequence.
    pub fn set_aside(&self, scope: &str, stream: &str, from: u64, why: &str) -> Result<u64> {
        let (scope, stream, why) = (scope.to_string(), stream.to_string(), why.to_string());
        let next = self.database().write_now_at(Priority::Normal, 4096, move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let (stream_id, next): (i64, i64) = db
                .prepare_cached("SELECT id, next FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?
                .ok_or_else(|| invalid("no such stream"))?;
            // A frame holds no more than `FRAME_RECORDS` records: the one
            // holding `from` begins no further before it than that.
            let cut: Option<i64> = db
                .prepare_cached(
                    "SELECT first FROM frames INDEXED BY frame_keys
                     WHERE stream = ?1 AND holding = ?2 AND first >= ?4 AND first + count > ?3 ORDER BY first LIMIT 1",
                )?
                .query_row(
                    params![stream_id, holding, from as i64, (from as i64).saturating_sub(crate::record::FRAME_RECORDS as i64 - 1)],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(cut) = cut else { return Ok(next as u64) };
            let mut after = cut - 1;
            loop {
                let frames: Vec<Aside> = {
                    let mut statement = db.prepare_cached(
                        "SELECT first, header, signature, body FROM frames
                         WHERE stream = ?1 AND holding = ?2 AND first > ?3 ORDER BY first LIMIT ?4",
                    )?;
                    let rows = statement.query_map(params![stream_id, holding, after, AT_ONCE], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })?;
                    rows.collect::<std::result::Result<_, _>>()?
                };
                let Some((last, ..)) = frames.last() else { break };
                after = *last;
                for (_, header, signature, body) in frames {
                    let decoded = Header::decode(&header).map_err(invalid)?;
                    // The objects its records refer to, held with it; those
                    // here are shown, being here.
                    let objects: Vec<crate::ObjectRef> = open(&decoded, &body)
                        .map_err(invalid)?
                        .into_iter()
                        .flat_map(|record| match record.record.state {
                            State::Is { objects, .. } => objects,
                            State::Gone => Vec::new(),
                        })
                        .collect();
                    let mut shown = BTreeSet::new();
                    for object in &objects {
                        if super::objects::present(db, &object.hash)?.is_some() {
                            shown.insert(object.hash);
                        }
                    }
                    let frame = RawFrame { header, signature, body, objects: Vec::new() };
                    super::handover::keep_evidence(db, (stream_id, holding), &decoded, &frame, &why, &objects, &shown)?;
                }
            }
            truncate_in(db, scope_id, holding, stream_id, (cut > 0).then(|| cut as u64 - 1))?;
            Ok(cut as u64)
        })?;
        self.wrote();
        Ok(next)
    }
}
