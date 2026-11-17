//! Reclaiming: an object goes once no record of any scope refers to it and
//! it was put a while ago.
//!
//! The while is what lets an object be put before the record that refers
//! to it is appended (`crate::objects`). A pass looks at a bounded number
//! of objects from where the last one stopped, so however many objects
//! there are, no pass is long.

use crate::{Content, Result};
use fastctx_storage::rusqlite::OptionalExtension;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The objects one pass looks at, at most.
const LOOKED: usize = 512;

/// What one pass of reclaiming did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    /// Objects looked at.
    pub looked: usize,
    /// Objects removed, and what an interrupted put left.
    pub removed: usize,
    /// The bytes of the objects removed.
    pub bytes: u64,
    /// Whether the pass stopped before the last object: another finds more.
    pub more: bool,
}

/// Where the passes are: the last object looked at, and whose turn it is
/// among the directories of files.
#[derive(Default)]
pub(crate) struct Passes {
    after: Vec<u8>,
    turn: u8,
}

impl Content {
    /// One pass: removes the objects, among those looked at, that were put
    /// more than `grace` ago and that nothing refers to. Call on a thread
    /// that may block.
    pub fn reclaim(&self, grace: Duration) -> Result<Reclaimed> {
        let before = SystemTime::now().checked_sub(grace).unwrap_or(UNIX_EPOCH);
        let cutoff = before.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64);
        let (after, turn) = {
            let passes = self.0.reclaiming.lock().expect("reclaiming passes");
            (passes.after.clone(), passes.turn)
        };
        let aged = self.0.objects.aged(&after, cutoff, LOOKED)?;
        let unreferred: Vec<[u8; 32]> = self.0.database.read_now(|db| {
            let mut referred = db.prepare_cached("SELECT 1 FROM object_refs WHERE hash = ?1 AND count > 0 LIMIT 1")?;
            let mut unreferred = Vec::new();
            for hash in &aged {
                if referred.query_row([&hash[..]], |_| Ok(())).optional()?.is_none() {
                    unreferred.push(*hash);
                }
            }
            Ok(unreferred)
        })?;
        let (removed, bytes) =
            if unreferred.is_empty() { (0, 0) } else { self.0.objects.drop_aged(unreferred, cutoff)? };
        let swept = self.0.objects.sweep(turn, before)?;
        let more = aged.len() == LOOKED;
        let mut passes = self.0.reclaiming.lock().expect("reclaiming passes");
        passes.after = if more { aged.last().map(|hash| hash.to_vec()).unwrap_or_default() } else { Vec::new() };
        passes.turn = turn.wrapping_add(1);
        Ok(Reclaimed { looked: aged.len(), removed: removed + swept, bytes, more })
    }
}
