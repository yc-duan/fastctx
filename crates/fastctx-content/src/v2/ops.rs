//! Operations of a true source (§3): each write a sender makes carries an
//! operation number and a digest of what it asked, committed in the same
//! transaction as the records it lands as. A row means the write is durable;
//! no row means it never arrived, and the sender sends it again under the
//! same number. The same number with the same digest is the same write,
//! answered as done; with another digest it is refused.
//!
//! Words for a host also carry where their delivery stands: `kept`, then
//! `delivering` — committed in full before they cross into the host, which
//! no lost power may undo — then `delivered` once the host's own record
//! shows them. Nothing is handed over again for want of seeing it.

use super::Library;
use crate::error::invalid;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// What refusing an operation number used for another request says: told
/// apart by those who answer the sender (§3).
pub const OP_REUSED: &str = "that operation number was used for another request";

/// A write's operation: its number, the digest of the request and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op {
    pub id: String,
    pub digest: [u8; 32],
    pub kind: String,
}

/// Where words stand on their way into the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Kept,
    Delivering,
    Delivered,
}

impl Delivery {
    fn name(self) -> &'static str {
        match self {
            Self::Kept => "kept",
            Self::Delivering => "delivering",
            Self::Delivered => "delivered",
        }
    }
}

/// Whether `op` was committed already: `Some(true)` for the same request,
/// an error for another under the same number, `None` when it is new.
pub(super) fn seen(db: &Connection, op: &Op) -> fastctx_storage::Result<bool> {
    let digest: Option<Vec<u8>> =
        db.prepare_cached("SELECT digest FROM ops WHERE op = ?1")?.query_row([&op.id], |row| row.get(0)).optional()?;
    match digest {
        Some(digest) if digest == op.digest => Ok(true),
        Some(_) => Err(invalid(OP_REUSED)),
        None => Ok(false),
    }
}

/// Records `op` as landed as the records `first` to `first + count - 1` of
/// `stream` in `scope`, words starting as kept.
pub(super) fn landed(
    db: &Connection,
    op: &Op,
    scope: i64,
    stream: &str,
    first: u64,
    count: u64,
    words: bool,
) -> fastctx_storage::Result<()> {
    db.prepare_cached("INSERT INTO ops (op, digest, kind, state, delivery, kept, why) VALUES (?1, ?2, ?3, 'durable', ?4, NULL, NULL)")?
        .execute(params![op.id, &op.digest[..], op.kind, words.then_some("kept")])?;
    let mut mapping = db.prepare_cached("INSERT OR REPLACE INTO op_records (scope, stream, revision, op) VALUES (?1, ?2, ?3, ?4)")?;
    for revision in first..first + count {
        mapping.execute(params![scope, stream, revision as i64, op.id])?;
    }
    Ok(())
}

/// The operation whose record stands at `revision` of `stream` in `scope`.
pub(super) fn op_at(db: &Connection, scope: i64, stream: &str, revision: u64) -> fastctx_storage::Result<Option<String>> {
    Ok(db
        .prepare_cached("SELECT op FROM op_records WHERE scope = ?1 AND stream = ?2 AND revision = ?3")?
        .query_row(params![scope, stream, revision as i64], |row| row.get(0))
        .optional()?)
}

/// A refused record of a true source's write is kept in its operation, with
/// why, for the user to see, drop or send again (§6.5).
pub(super) fn refused_in(db: &Connection, op: &str, seq: i64, record: &[u8], why: &str) -> fastctx_storage::Result<()> {
    let kept: Option<Vec<u8>> =
        db.prepare_cached("SELECT kept FROM ops WHERE op = ?1")?.query_row([op], |row| row.get(0)).optional()?.flatten();
    // Each record in its place in the order it was first written in, however
    // many times it was refused apart (T06).
    let mut pieces = pieces(&kept.unwrap_or_default()).map_err(invalid)?;
    let at = pieces.partition_point(|(placed, _)| *placed <= seq);
    pieces.insert(at, (seq, record.to_vec()));
    let mut kept = KEPT.to_vec();
    for (seq, record) in &pieces {
        kept.extend_from_slice(&seq.to_le_bytes());
        kept.extend_from_slice(&(record.len() as u32).to_le_bytes());
        kept.extend_from_slice(record);
    }
    db.prepare_cached("UPDATE ops SET state = 'refused', kept = ?2, why = ?3 WHERE op = ?1")?.execute(params![op, kept, why])?;
    Ok(())
}

/// What begins kept records that carry their places in the order. Read as
/// the length an older kept record begins with, it is past any record's
/// (U06): no older kept record begins so.
const KEPT: &[u8] = b"K2\xff\xff";

/// The kept records of an operation with their places in the order; those
/// kept before places were, in the order they were kept.
fn pieces(kept: &[u8]) -> std::result::Result<Vec<(i64, Vec<u8>)>, String> {
    let (placed, mut cursor) = match kept.strip_prefix(KEPT) {
        Some(rest) => (true, rest),
        None => (false, kept),
    };
    let mut pieces = Vec::new();
    while !cursor.is_empty() {
        let seq = if placed {
            let seq = i64::from_le_bytes(cursor.get(..8).ok_or("a kept record's place is cut short")?.try_into().expect("eight bytes"));
            cursor = &cursor[8..];
            seq
        } else {
            pieces.len() as i64
        };
        let length = u32::from_le_bytes(cursor.get(..4).ok_or("a kept record is cut short")?.try_into().expect("four bytes")) as usize;
        let record = cursor.get(4..4 + length).ok_or("a kept record is cut short")?;
        pieces.push((seq, record.to_vec()));
        cursor = &cursor[4 + length..];
    }
    Ok(pieces)
}

impl Library {
    /// Which of `ops` are durable here.
    pub fn durability(&self, ops: Vec<String>) -> Result<Vec<(String, bool)>> {
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached("SELECT 1 FROM ops WHERE op = ?1")?;
            ops.into_iter()
                .map(|op| {
                    let durable = statement.query_row([&op], |_| Ok(())).optional()?.is_some();
                    Ok((op, durable))
                })
                .collect()
        })?)
    }

    /// Moves words along their delivery: `Delivering` waits for the disk
    /// before it returns, as it must before they cross into the host.
    pub fn deliver(&self, op: &str, delivery: Delivery) -> Result<()> {
        let op = op.to_string();
        let write = move |db: &Connection| -> fastctx_storage::Result<()> {
            let changed = db
                .prepare_cached("UPDATE ops SET delivery = ?2 WHERE op = ?1 AND delivery IS NOT NULL")?
                .execute(params![op, delivery.name()])?;
            if changed == 0 {
                return Err(invalid("no words of that operation are kept"));
            }
            Ok(())
        };
        match delivery {
            Delivery::Delivering => self.database().write_now_at(Priority::Interactive, 128, write)?,
            _ => self.database().write_relaxed_now_at(Priority::Normal, 128, write)?,
        }
        Ok(())
    }

    /// What of `op` could not be written, and why: its records as the user
    /// wrote them, kept until the user drops it or sends it again.
    pub fn refused(&self, op: &str) -> Result<Option<(Vec<super::frame::KindRecord>, String)>> {
        let op = op.to_string();
        Ok(self.database().read_now(move |db| {
            let found: Option<(Option<Vec<u8>>, Option<String>)> = db
                .prepare_cached("SELECT kept, why FROM ops WHERE op = ?1 AND state = 'refused'")?
                .query_row([&op], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((kept, why)) = found else { return Ok(None) };
            let mut records = Vec::new();
            for (_, record) in pieces(&kept.unwrap_or_default()).map_err(invalid)? {
                records.push(super::rejected::decode(&record).map_err(invalid)?);
            }
            Ok(Some((records, why.unwrap_or_default())))
        })?)
    }

    /// Where the words of `op` stand, if it carried words.
    pub fn delivery(&self, op: &str) -> Result<Option<Delivery>> {
        let op = op.to_string();
        Ok(self.database().read_now(move |db| {
            let state: Option<Option<String>> = db
                .prepare_cached("SELECT delivery FROM ops WHERE op = ?1")?
                .query_row([&op], |row| row.get(0))
                .optional()?;
            Ok(match state.flatten().as_deref() {
                Some("kept") => Some(Delivery::Kept),
                Some("delivering") => Some(Delivery::Delivering),
                Some("delivered") => Some(Delivery::Delivered),
                _ => None,
            })
        })?)
    }
}
