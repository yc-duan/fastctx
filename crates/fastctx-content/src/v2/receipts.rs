//! The hub's receipts for a producer's frames (§6.5, §17).
//!
//! A frame is stored as `parked` (kept, not yet applied, with the scope's
//! parked sequence and accumulator), `applied` (with its change number) or
//! `rejected`. A receipt stays unwitnessed until the hub says a checkpoint
//! written outside its directory covers it; until then the device reports
//! its latest unwitnessed receipt of each scope when it opens a session, and
//! keeps the frames it covers (§4.6, Q06, Q07).

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
    Applied { change: u64 },
    Rejected,
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
    /// Keeps a receipt of `scope`; a later word on the same frame replaces
    /// an earlier one (parked, then applied or rejected).
    pub fn receipt(&self, scope: &str, receipt: Receipt) -> Result<()> {
        let scope = scope.to_string();
        Ok(self.database().write_relaxed_now_at(Priority::Normal, 256 + receipt.signature.len(), move |db| {
            let Some((scope_id, _, _)) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let stream: i64 = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, receipt.stream], |row| row.get(0))
                .optional()?
                .ok_or_else(|| invalid("a receipt for a stream not here"))?;
            let (kind, change, parked, accumulated) = match &receipt.said {
                Said::Parked { sequence, accumulated } => ("parked", None, Some(*sequence as i64), Some(accumulated.to_vec())),
                Said::Applied { change } => ("applied", Some(*change as i64), None, None),
                Said::Rejected => ("rejected", None, None, None),
            };
            db.prepare_cached(
                "INSERT INTO hub_receipts (stream, epoch, first, digest, kind, change, parked, accumulated, signature, checkpointed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)
                 ON CONFLICT (stream, epoch, first, digest) DO UPDATE SET
                    kind = excluded.kind,
                    change = COALESCE(excluded.change, hub_receipts.change),
                    parked = COALESCE(hub_receipts.parked, excluded.parked),
                    accumulated = COALESCE(hub_receipts.accumulated, excluded.accumulated),
                    signature = excluded.signature,
                    checkpointed = CASE WHEN hub_receipts.kind = excluded.kind THEN hub_receipts.checkpointed ELSE 0 END",
            )?
            .execute(params![
                stream,
                receipt.epoch as i64,
                receipt.first as i64,
                &receipt.digest[..],
                kind,
                change,
                parked,
                accumulated,
                receipt.signature
            ])?;
            if kind == "applied" {
                db.prepare_cached(
                    "INSERT INTO outbox (stream, uploaded) VALUES (?1, ?2)
                     ON CONFLICT (stream) DO UPDATE SET uploaded = MAX(uploaded, excluded.uploaded)",
                )?
                .execute(params![stream, receipt.first as i64])?;
            }
            Ok(())
        })?)
    }

    /// The hub says a checkpoint outside its directory covers `scope`'s
    /// changes up to `change` and its parked frames up to `parked`.
    pub fn checkpointed(&self, scope: &str, change: u64, parked: u64) -> Result<usize> {
        let scope = scope.to_string();
        Ok(self.database().write_relaxed_now_at(Priority::Normal, 128, move |db| {
            let Some((scope_id, _, _)) = held(db, &scope)? else { return Ok(0) };
            Ok(db
                .prepare_cached(
                    "UPDATE hub_receipts SET checkpointed = 1 WHERE checkpointed = 0
                     AND stream IN (SELECT id FROM streams WHERE scope = ?1)
                     AND ((kind = 'applied' AND change <= ?2) OR (kind = 'parked' AND parked <= ?3))",
                )?
                .execute(params![scope_id, change as i64, parked as i64])?)
        })?)
    }

    /// The latest unwitnessed applied and parked receipt of each scope here,
    /// for the hub to check when a session opens (§17).
    pub fn unwitnessed(&self) -> Result<Vec<(String, Receipt)>> {
        Ok(self.database().read_now(|db| {
            let mut statement = db.prepare_cached(
                "SELECT c.key, s.name, r.epoch, r.first, r.digest, r.kind, r.change, r.parked, r.accumulated, r.signature
                 FROM hub_receipts r JOIN streams s ON s.id = r.stream JOIN scopes c ON c.id = s.scope
                 WHERE r.checkpointed = 0 AND r.kind IN ('applied', 'parked')
                   AND NOT EXISTS (
                     SELECT 1 FROM hub_receipts later JOIN streams ls ON ls.id = later.stream
                     WHERE ls.scope = s.scope AND later.checkpointed = 0 AND later.kind = r.kind
                       AND ((r.kind = 'applied' AND later.change > r.change) OR (r.kind = 'parked' AND later.parked > r.parked)))",
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
                    row.get::<_, Option<Vec<u8>>>(8)?,
                    row.get::<_, Vec<u8>>(9)?,
                ))
            })?;
            let mut reported = Vec::new();
            for row in rows {
                let (scope, stream, epoch, first, digest, kind, change, parked, accumulated, signature) = row?;
                let said = match kind.as_str() {
                    "applied" => Said::Applied { change: change.unwrap_or_default() as u64 },
                    _ => Said::Parked {
                        sequence: parked.unwrap_or_default() as u64,
                        accumulated: accumulated.unwrap_or_default().try_into().unwrap_or([0; 32]),
                    },
                };
                let digest = digest.try_into().map_err(|_| invalid("a malformed receipt digest"))?;
                reported.push((scope, Receipt { stream, epoch: epoch as u64, first: first as u64, digest, said, signature }));
            }
            Ok(reported)
        })?)
    }
}
