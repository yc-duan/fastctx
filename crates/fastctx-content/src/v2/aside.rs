//! A replica's frames of a stream past where the hub holds it now, set
//! aside rather than lost (fff AT02, AT03). After the hub was put back from
//! an older copy, the streams it orders it writes again from its records,
//! which may no longer hold all it once signed; this machine's frames from
//! the first that reaches past the hub's place on go into `conflicts`, each
//! whole, and the stream is cut back before that frame: at the end of one
//! of its own frames, or to its start.

use super::Library;
use super::objects::now;
use super::scopes::held;
use super::window::truncate_in;
use crate::Result;
use crate::error::invalid;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};

/// A frame of a stream set aside: its epoch, its first sequence, its
/// header, signature and body.
type Aside = (i64, i64, Vec<u8>, Vec<u8>, Vec<u8>);

/// A frame as it is kept aside: its header's length and its signature's,
/// each four bytes big-endian, then the header, the signature and the body.
fn whole(header: &[u8], signature: &[u8], body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + header.len() + signature.len() + body.len());
    bytes.extend_from_slice(&(header.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&(signature.len() as u32).to_be_bytes());
    bytes.extend_from_slice(header);
    bytes.extend_from_slice(signature);
    bytes.extend_from_slice(body);
    bytes
}

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
            let cut: Option<i64> = db
                .prepare_cached("SELECT MIN(first) FROM frames WHERE stream = ?1 AND holding = ?2 AND first + count > ?3")?
                .query_row(params![stream_id, holding, from as i64], |row| row.get(0))?;
            let Some(cut) = cut else { return Ok(next as u64) };
            let frames: Vec<Aside> = {
                let mut statement = db.prepare_cached(
                    "SELECT epoch, first, header, signature, body FROM frames
                     WHERE stream = ?1 AND holding = ?2 AND first >= ?3 ORDER BY first",
                )?;
                let rows = statement.query_map(params![stream_id, holding, cut], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                })?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let at = now();
            for (epoch, first, header, signature, body) in frames {
                db.prepare_cached("INSERT INTO conflicts (stream, epoch, first, why, bytes, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
                    .execute(params![stream_id, epoch, first, why, whole(&header, &signature, &body), at])?;
            }
            truncate_in(db, scope_id, holding, stream_id, (cut > 0).then(|| cut as u64 - 1))?;
            Ok(cut as u64)
        })?;
        self.wrote();
        Ok(next)
    }
}
