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

    /// Every unwitnessed applied and parked receipt here, for the hub to
    /// check when a session opens (§17): few, as each checkpoint the hub
    /// confirms takes those it covers off.
    pub fn unwitnessed(&self) -> Result<Vec<(String, Receipt)>> {
        Ok(self.database().read_now(|db| {
            let mut statement = db.prepare_cached(
                "SELECT c.key, s.name, r.epoch, r.first, r.digest, r.kind, r.change, r.parked, r.standing, r.signature
                 FROM hub_receipts r JOIN streams s ON s.id = r.stream JOIN scopes c ON c.id = s.scope
                 WHERE r.checkpointed = 0 AND r.kind IN ('applied', 'parked')
                 ORDER BY c.key, r.kind, COALESCE(r.change, r.parked)",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Vec<u8>>(8)?,
                    row.get::<_, Vec<u8>>(9)?,
                ))
            })?;
            let mut reported = Vec::new();
            for row in rows {
                let (scope, stream, epoch, first, digest, kind, change, parked, standing, signature) = row?;
                let standing: [u8; 32] = standing.try_into().map_err(|_| invalid("a malformed receipt"))?;
                let said = match kind.as_str() {
                    "applied" => Said::Applied { change: change.unwrap_or_default() as u64, prefix: standing },
                    _ => Said::Parked { sequence: parked.unwrap_or_default() as u64, accumulated: standing },
                };
                let digest = digest.try_into().map_err(|_| invalid("a malformed receipt digest"))?;
                reported.push((scope, Receipt { stream, epoch: epoch as u64, first: first as u64, digest, said, signature }));
            }
            Ok(reported)
        })?)
    }
}
