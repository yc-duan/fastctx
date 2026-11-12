//! The hub's receipts for a producer's frames (§6.5, §17).
//!
//! A frame is stored as `parked` (kept, not yet applied, with the scope's
//! parked sequence and accumulator), `applied` (with its change number and
//! the scope's change prefix there) or `rejected`; each word is kept with
//! its own signature, so a parked receipt stays a witness after the frame
//! is applied, and words on one frame from two histories of the hub — a
//! late one from before a restore beside one from after it — are two
//! receipts, told apart by what each stands on (T01).
//!
//! A receipt stays unwitnessed until the hub says a checkpoint written
//! outside its directory covers it; until then the device reports it when
//! it opens a session — every one, not only the latest: one from a history
//! the hub has lost may have a smaller number than one from the history it
//! has — and keeps the frames it covers (§4.6, §17). A checkpoint's word
//! takes off only the receipts the hub stands behind in the session it
//! comes in: those it gave there, and those reported when it opened that
//! the hub checked. One that came any other way waits to be reported.

use super::Library;
use super::scopes::held;
use crate::error::invalid;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};

/// What the hub said of one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    Parked { sequence: u64, accumulated: [u8; 32] },
    Applied { change: u64, prefix: [u8; 32] },
    /// Refused with every later frame of its epoch from `from`.
    Rejected { from: u64 },
}

impl Said {
    /// As kept: its kind, change number, parked sequence, and what it
    /// stands on.
    fn kept(&self) -> (&'static str, Option<i64>, Option<i64>, Vec<u8>) {
        match self {
            Said::Parked { sequence, accumulated } => ("parked", None, Some(*sequence as i64), accumulated.to_vec()),
            Said::Applied { change, prefix } => ("applied", Some(*change as i64), None, prefix.to_vec()),
            Said::Rejected { from } => ("rejected", None, None, from.to_le_bytes().to_vec()),
        }
    }
}

/// Where a report of unwitnessed receipts goes on from: a long backlog is
/// read a page at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    stream: i64,
    epoch: i64,
    first: i64,
    digest: Vec<u8>,
    kind: String,
    standing: Vec<u8>,
}

/// A page of unwitnessed receipts, each with its scope, and where the next
/// page starts if there may be one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub receipts: Vec<(String, Receipt)>,
    pub next: Option<Mark>,
}

/// One receipt, as kept and as reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub stream: String,
    pub epoch: u64,
    pub first: u64,
    pub digest: [u8; 32],
    pub said: Said,
    pub signature: Vec<u8>,
}

impl Library {
    /// Keeps a receipt of `scope` heard in `session`, a number no other
    /// session of this library has. A parked or applied frame counts as
    /// uploaded: the hub keeps its bytes.
    pub fn receipt(&self, scope: &str, receipt: Receipt, session: u64) -> Result<()> {
        let scope = scope.to_string();
        Ok(self.database().write_relaxed_now_at(Priority::Normal, 256 + receipt.signature.len(), move |db| {
            let Some((scope_id, _, _)) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let stream: i64 = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, receipt.stream], |row| row.get(0))
                .optional()?
                .ok_or_else(|| invalid("a receipt for a stream not here"))?;
            let (kind, change, parked, standing) = receipt.said.kept();
            // The same word heard again stands behind it in this session.
            db.prepare_cached(
                "INSERT INTO hub_receipts (stream, epoch, first, digest, kind, standing, change, parked, signature, heard_in, checkpointed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)
                 ON CONFLICT (stream, epoch, first, digest, kind, standing) DO UPDATE SET heard_in = excluded.heard_in",
            )?
            .execute(params![
                stream,
                receipt.epoch as i64,
                receipt.first as i64,
                &receipt.digest[..],
                kind,
                standing,
                change,
                parked,
                receipt.signature,
                session as i64
            ])?;
            if kind != "rejected" {
                let count: i64 = db
                    .prepare_cached("SELECT count FROM frames WHERE stream = ?1 AND epoch = ?2 AND first = ?3 LIMIT 1")?
                    .query_row(params![stream, receipt.epoch as i64, receipt.first as i64], |row| row.get(0))
                    .optional()?
                    .unwrap_or(1);
                // Where uploading goes on from: past the frame.
                db.prepare_cached(
                    "INSERT INTO outbox (stream, uploaded) VALUES (?1, ?2)
                     ON CONFLICT (stream) DO UPDATE SET uploaded = MAX(uploaded, excluded.uploaded)",
                )?
                .execute(params![stream, receipt.first as i64 + count])?;
            }
            Ok(())
        })?)
    }

    /// The hub checked these receipts, reported when `session` opened: a
    /// checkpoint's word in that session may take them off. How many.
    pub fn vouched(&self, session: u64, receipts: Vec<(String, Receipt)>) -> Result<usize> {
        Ok(self.database().write_relaxed_now_at(Priority::Normal, 128 + receipts.len() * 128, move |db| {
            let mut vouched = 0;
            for (scope, receipt) in &receipts {
                let (kind, _, _, standing) = receipt.said.kept();
                vouched += db
                    .prepare_cached(
                        "UPDATE hub_receipts SET heard_in = ?1 WHERE checkpointed = 0 AND kind = ?2 AND standing = ?3
                         AND epoch = ?4 AND first = ?5 AND digest = ?6 AND stream =
                         (SELECT s.id FROM streams s JOIN scopes c ON c.id = s.scope WHERE c.key = ?7 AND s.name = ?8)",
                    )?
                    .execute(params![
                        session as i64,
                        kind,
                        standing,
                        receipt.epoch as i64,
                        receipt.first as i64,
                        &receipt.digest[..],
                        scope,
                        receipt.stream
                    ])?;
            }
            Ok(vouched)
        })?)
    }

    /// The hub says in `session` that a checkpoint outside its directory
    /// covers `scope`'s changes up to `change` and its parked frames up to
    /// `parked`.
    pub fn checkpointed(&self, scope: &str, change: u64, parked: u64, session: u64) -> Result<usize> {
        let scope = scope.to_string();
        Ok(self.database().write_relaxed_now_at(Priority::Normal, 128, move |db| {
            let Some((scope_id, _, _)) = held(db, &scope)? else { return Ok(0) };
            Ok(db
                .prepare_cached(
                    "UPDATE hub_receipts SET checkpointed = 1 WHERE checkpointed = 0 AND heard_in = ?4
                     AND stream IN (SELECT id FROM streams WHERE scope = ?1)
                     AND ((kind = 'applied' AND change <= ?2) OR (kind = 'parked' AND parked <= ?3))",
                )?
                .execute(params![scope_id, change as i64, parked as i64, session as i64])?)
        })?)
    }

    /// The unwitnessed applied and parked receipts here after `after`, up
    /// to `limit`, for the hub to check when a session opens (§17). Every
    /// one is reported; they are few, as each checkpoint the hub confirms
    /// takes those it covers off, but a long backlog is read a page at a
    /// time.
    pub fn unwitnessed(&self, after: Option<&Mark>, limit: usize) -> Result<Report> {
        let after = after.cloned();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT c.key, s.name, r.stream, r.epoch, r.first, r.digest, r.kind, r.standing, r.change, r.parked, r.signature
                 FROM hub_receipts r JOIN streams s ON s.id = r.stream JOIN scopes c ON c.id = s.scope
                 WHERE r.checkpointed = 0 AND r.kind IN ('applied', 'parked')
                   AND (?1 IS NULL OR (r.stream, r.epoch, r.first, r.digest, r.kind, r.standing) > (?1, ?2, ?3, ?4, ?5, ?6))
                 ORDER BY r.stream, r.epoch, r.first, r.digest, r.kind, r.standing LIMIT ?7",
            )?;
            let at = after.as_ref();
            let rows = statement.query_map(
                params![
                    at.map(|mark| mark.stream),
                    at.map(|mark| mark.epoch),
                    at.map(|mark| mark.first),
                    at.map(|mark| mark.digest.clone()),
                    at.map(|mark| mark.kind.clone()),
                    at.map(|mark| mark.standing.clone()),
                    limit as i64
                ],
                |row| {
                    Ok((
                        (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                        Mark {
                            stream: row.get(2)?,
                            epoch: row.get(3)?,
                            first: row.get(4)?,
                            digest: row.get(5)?,
                            kind: row.get(6)?,
                            standing: row.get(7)?,
                        },
                        (row.get::<_, Option<i64>>(8)?, row.get::<_, Option<i64>>(9)?, row.get::<_, Vec<u8>>(10)?),
                    ))
                },
            )?;
            let (mut reported, mut last) = (Vec::new(), None);
            for row in rows {
                let ((scope, stream), mark, (change, parked, signature)) = row?;
                let standing: [u8; 32] = mark.standing.clone().try_into().map_err(|_| invalid("a malformed receipt"))?;
                let said = match mark.kind.as_str() {
                    "applied" => Said::Applied { change: change.unwrap_or_default() as u64, prefix: standing },
                    _ => Said::Parked { sequence: parked.unwrap_or_default() as u64, accumulated: standing },
                };
                let digest = mark.digest.clone().try_into().map_err(|_| invalid("a malformed receipt digest"))?;
                reported.push((scope, Receipt { stream, epoch: mark.epoch as u64, first: mark.first as u64, digest, said, signature }));
                last = Some(mark);
            }
            let next = if reported.len() == limit { last } else { None };
            Ok(Report { receipts: reported, next })
        })?)
    }
}
