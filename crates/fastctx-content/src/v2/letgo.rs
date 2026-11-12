//! A device letting go of what it may hold no more (design §2.3, §4.5):
//! what of its own the hub has not taken yet is kept to commit again first,
//! in the same transaction as what is let go, so nothing committed meanwhile
//! goes with the rest.

use super::Library;
use super::scopes::held;
use crate::Result;
use crate::error::invalid;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// This device's own streams of `scope_id` in `holding`, each with what the
/// hub has not taken of it moved into the records waiting to be committed
/// again; the stream named `only`, when one is.
fn keep_unsent(db: &Connection, scope_id: i64, holding: i64, device: &[u8; 32], only: Option<i64>) -> fastctx_storage::Result<()> {
    for (stream, name, next) in super::device::own(db, scope_id, holding, device)? {
        if only.is_some_and(|only| only != stream) {
            continue;
        }
        let uploaded: i64 =
            db.prepare_cached("SELECT uploaded FROM outbox WHERE stream = ?1")?.query_row([stream], |row| row.get(0)).optional()?.unwrap_or(0);
        if uploaded < next {
            super::rejected::rejected_in(db, scope_id, holding, stream, &name, uploaded as u64)?;
        }
    }
    Ok(())
}

impl Library {
    /// Lets `stream` of `scope` go, as one this device may hold no more:
    /// what of it is this device's own and not yet taken by the hub is kept
    /// to commit again, then the stream is emptied, in one transaction.
    pub fn withhold(&self, scope: &str, stream: &str, device: &[u8; 32]) -> Result<()> {
        let (scope, stream, device) = (scope.to_string(), stream.to_string(), *device);
        Ok(self.database().write_now_at(Priority::Normal, 1 << 20, move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let Some(stream_id) = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get::<_, i64>(0))
                .optional()?
            else {
                return Ok(());
            };
            keep_unsent(db, scope_id, holding, &device, Some(stream_id))?;
            super::window::truncate_in(db, scope_id, holding, stream_id, None)
        })?)
    }

    /// Revokes `scope` here as `revoke` does, what this device's own streams
    /// hold that the hub has not taken kept first to commit again, in the
    /// same transaction: when the period's purge comes, it is kept for its
    /// user (§3, §4.5).
    pub fn revoke_keeping(&self, scope: &str, device: &[u8; 32]) -> Result<()> {
        let (scope, device) = (scope.to_string(), *device);
        Ok(self.database().write_now_at(Priority::Control, 1 << 20, move |db| {
            if let Some((scope_id, _, Some(holding))) = held(db, &scope)? {
                keep_unsent(db, scope_id, holding, &device, None)?;
            }
            super::scopes::revoke_in(db, &scope)
        })?)
    }
}
