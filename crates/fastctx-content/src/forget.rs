//! Forgetting a scope: everything kept of it goes, a bounded part at a
//! time, so a scope of any size is forgotten without one long write.
//!
//! Its records and items go first, then what it counted of objects; an
//! object nothing refers to any more goes with it unless it was put just
//! now, when a record of another scope may be about to refer to it
//! (`crate::reclaim` takes that one later).

use crate::{Content, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Items one pass removes, at most.
const ITEMS: i64 = 4096;
/// Frames one pass removes, at most.
const FRAMES: i64 = 256;
/// Objects one pass stops counting, at most.
const OBJECTS: i64 = 512;
/// An object put within this is left for reclaiming: a record may be about
/// to refer to it.
const SETTLED: Duration = Duration::from_secs(60);

/// What one pass of forgetting removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Forgotten {
    pub items: usize,
    pub frames: usize,
    /// Objects removed because nothing refers to them any more.
    pub objects: usize,
    /// Whether more of the scope is left: another pass removes it.
    pub more: bool,
}

impl Content {
    /// One pass of forgetting `scope`. Call on a thread that may block, and
    /// again while `more`.
    pub fn forget(&self, scope: &str) -> Result<Forgotten> {
        let key = scope.to_string();
        let (forgotten, unreferred) = self.0.database.write_now_at(Priority::Background, 4096, move |db| {
            let nothing = (Forgotten::default(), Vec::new());
            let id: Option<i64> = db
                .prepare_cached("SELECT id FROM scopes WHERE key = ?1")?
                .query_row([&key], |row| row.get(0))
                .optional()?;
            let Some(id) = id else { return Ok(nothing) };
            let items = db
                .prepare_cached(
                    "DELETE FROM items WHERE (stream, place, item) IN (SELECT stream, place, item FROM items \
                     WHERE stream IN (SELECT id FROM streams WHERE scope = ?1) LIMIT ?2)",
                )?
                .execute(params![id, ITEMS])?;
            if items > 0 {
                return Ok((Forgotten { items, more: true, ..Forgotten::default() }, Vec::new()));
            }
            let frames = db
                .prepare_cached(
                    "DELETE FROM frames WHERE (stream, first) IN (SELECT stream, first FROM frames \
                     WHERE stream IN (SELECT id FROM streams WHERE scope = ?1) LIMIT ?2)",
                )?
                .execute(params![id, FRAMES])?;
            if frames > 0 {
                return Ok((Forgotten { frames, more: true, ..Forgotten::default() }, Vec::new()));
            }
            let counted: Vec<Vec<u8>> = db
                .prepare_cached("SELECT hash FROM object_refs WHERE scope = ?1 LIMIT ?2")?
                .query_map(params![id, OBJECTS], |row| row.get(0))?
                .collect::<fastctx_storage::rusqlite::Result<_>>()?;
            if !counted.is_empty() {
                let mut uncount = db.prepare_cached("DELETE FROM object_refs WHERE scope = ?1 AND hash = ?2")?;
                let mut referred =
                    db.prepare_cached("SELECT 1 FROM object_refs WHERE hash = ?1 AND count > 0 LIMIT 1")?;
                let mut unreferred = Vec::new();
                for hash in counted {
                    uncount.execute(params![id, hash])?;
                    if referred.query_row([&hash], |_| Ok(())).optional()?.is_none()
                        && let Ok(hash) = <[u8; 32]>::try_from(hash)
                    {
                        unreferred.push(hash);
                    }
                }
                return Ok((Forgotten { more: true, ..Forgotten::default() }, unreferred));
            }
            db.prepare_cached("DELETE FROM streams WHERE scope = ?1")?.execute([id])?;
            db.prepare_cached("DELETE FROM scopes WHERE id = ?1")?.execute([id])?;
            Ok(nothing)
        })?;
        let objects = if unreferred.is_empty() {
            0
        } else {
            let before = SystemTime::now().checked_sub(SETTLED).unwrap_or(UNIX_EPOCH);
            let before = before.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64);
            self.0.objects.drop_aged(unreferred, before)?.0
        };
        Ok(Forgotten { objects, ..forgotten })
    }

    /// Drops a producer's position, as when what it produced for is gone.
    pub fn drop_checkpoint(&self, name: &str) -> Result<()> {
        let name = name.to_string();
        Ok(self.0.database.write_now_at(Priority::Background, 256, move |db| {
            db.prepare_cached("DELETE FROM checkpoints WHERE name = ?1")?.execute([&name])?;
            Ok(())
        })?)
    }
}
