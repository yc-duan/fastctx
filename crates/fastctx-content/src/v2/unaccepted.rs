//! What a device wrote of a stream before the hub took anything of it, in a
//! scope it began itself before the hub described it (design §28.10, AH01):
//! a conversation that came here from another machine, begun here after
//! waiting in vain for the hub's description. Where the hub had the stream
//! from that machine all along, what was written here is let go — made
//! again from its sources once this device orders the stream — and the
//! stream is followed as a replica's until it is handed over.

use super::Library;
use super::scopes::held;
use crate::error::invalid;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// Whether `stream`, ordered by `device` in `holding`, is in its first
/// epoch, with nothing of it uploaded and no descriptor of the hub's for it
/// here.
fn never_taken(db: &Connection, scope: i64, holding: i64, stream: i64, device: &[u8; 32]) -> fastctx_storage::Result<bool> {
    let epoch: i64 = db.prepare_cached("SELECT epoch FROM streams WHERE id = ?1")?.query_row([stream], |row| row.get(0))?;
    let known: bool =
        db.prepare_cached("SELECT EXISTS (SELECT 1 FROM known_epochs WHERE stream = ?1)")?.query_row([stream], |row| row.get(0))?;
    let uploaded: i64 = db
        .prepare_cached("SELECT uploaded FROM outbox WHERE stream = ?1")?
        .query_row([stream], |row| row.get(0))
        .optional()?
        .unwrap_or(0);
    let own = super::device::own(db, scope, holding, device)?.into_iter().any(|(id, _, _)| id == stream);
    Ok(epoch == 0 && !known && uploaded == 0 && own)
}

impl Library {
    /// The streams of `scope` this device, `device`, wrote that the hub
    /// never took anything of, each with where it reaches.
    pub fn unaccepted(&self, scope: &str, device: &[u8; 32]) -> Result<Vec<(String, u64)>> {
        let (scope, device) = (scope.to_string(), *device);
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            let mut found = Vec::new();
            for (stream, name, next) in super::device::own(db, scope_id, holding, &device)? {
                if next > 0 && never_taken(db, scope_id, holding, stream, &device)? {
                    found.push((name, next as u64));
                }
            }
            Ok(found)
        })?)
    }

    /// Lets go what this device, `device`, wrote of `stream` of `scope` that
    /// the hub never took, with where its producer `checkpoint` stood, in one
    /// transaction: the stream empty here, its frames and index rows gone,
    /// read anew. Whether it did: written to meanwhile so the hub took some,
    /// it is not.
    pub fn discard_unaccepted(&self, scope: &str, stream: &str, device: &[u8; 32], checkpoint: Option<&str>) -> Result<bool> {
        let (scope, stream, device, checkpoint) = (scope.to_string(), stream.to_string(), *device, checkpoint.map(str::to_string));
        let discarded = self.database().write_now_at(Priority::Normal, 4096, move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let Some(stream_id) = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get::<_, i64>(0))
                .optional()?
            else {
                return Ok(false);
            };
            if !never_taken(db, scope_id, holding, stream_id, &device)? {
                return Ok(false);
            }
            super::window::truncate_in(db, scope_id, holding, stream_id, None)?;
            db.prepare_cached("DELETE FROM epochs WHERE stream = ?1")?.execute([stream_id])?;
            db.prepare_cached("DELETE FROM outbox WHERE stream = ?1")?.execute([stream_id])?;
            if let Some(name) = &checkpoint {
                db.prepare_cached("DELETE FROM checkpoints WHERE name = ?1")?.execute([name])?;
            }
            Ok(true)
        })?;
        if discarded {
            self.wrote();
        }
        Ok(discarded)
    }
}
