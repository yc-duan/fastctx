//! A node's source adapters share one pool and reserve a whole working window
//! before buffering. They never wait for growth while holding an earlier window.
use super::{Error, source_step::SourceStep};
use crate::{
    admission::{ByteBudget, BytePermit},
    identity::{ScopeKey, SourceId},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::time::Instant;

pub const DEFAULT_INGEST_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_INGEST_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: usize = 12 * 1024 * 1024;
pub const FLUSH_BYTES: usize = 4 * 1024 * 1024;

type SourceKey = (ScopeKey, SourceId);
struct Pool {
    budget: ByteBudget,
    sources: Mutex<BTreeMap<SourceKey, Weak<SourceSlot>>>,
}
struct SourceSlot {
    pool: Weak<Pool>,
    key: SourceKey,
    window: ByteBudget,
}
/// Service composition creates one pool per node, shared across all hubs and
/// source writers. Interactive reads use their own content budget.
#[derive(Clone)]
pub struct IngestBudget {
    pool: Arc<Pool>,
}
/// A narrow handle for one primary source. Clones, steps and host generations
/// all share the same single-window slot.
#[derive(Clone)]
pub struct SourceMemory {
    pool: Arc<Pool>,
    slot: Arc<SourceSlot>,
}
/// Owns admission for the complete input, pending content, indexes and scratch
/// buffers of a source step. Move it with work that outlives the adapter call.
pub struct SourceWindow {
    global: BytePermit,
    _source: BytePermit,
    _slot: Arc<SourceSlot>,
}
pub enum SourceAdmission {
    Granted(SourceWindow),
    /// Finish or release the earlier working set before requesting another.
    ReleaseRequired,
}
impl IngestBudget {
    /// Explicit configuration accepts 12..=64 MiB, enough for a full source
    /// window without consuming an unbounded share of machine memory.
    pub fn new(bytes: usize) -> Result<Self, Error> {
        if !(MAX_SOURCE_BYTES..=MAX_INGEST_BYTES).contains(&bytes) {
            return Err(Error::Invalid("ingest budget must be between 12 and 64 MiB".into()));
        }
        Ok(Self { pool: Arc::new(Pool { budget: ByteBudget::new(bytes)?, sources: Mutex::new(BTreeMap::new()) }) })
    }
    /// Resource probing belongs to service composition, not to this pool.
    /// Unknown resources use 32 MiB; detected memory/64 is clamped to 12..=32 MiB.
    pub fn for_machine(physical_bytes: Option<u64>) -> Result<Self, Error> {
        let bytes = physical_bytes
            .map(|bytes| (bytes / 64).clamp(MAX_SOURCE_BYTES as u64, DEFAULT_INGEST_BYTES as u64) as usize)
            .unwrap_or(DEFAULT_INGEST_BYTES);
        Self::new(bytes)
    }
    /// This only grants memory admission. The writer independently rechecks
    /// step, holding and content authority at its actual IO and commit ports.
    pub fn source(&self, step: &SourceStep) -> Result<SourceMemory, Error> {
        let primary = step.primary();
        let key = (primary.custody.object.scope.clone(), primary.source.clone());
        let mut sources = self.pool.sources.lock().map_err(|_| registry_unavailable())?;
        let slot = match sources.get(&key).and_then(Weak::upgrade) {
            Some(slot) => slot,
            None => {
                let slot = Arc::new(SourceSlot {
                    pool: Arc::downgrade(&self.pool),
                    key: key.clone(),
                    window: ByteBudget::new(MAX_SOURCE_BYTES)?,
                });
                sources.insert(key, Arc::downgrade(&slot));
                slot
            }
        };
        drop(sources);
        Ok(SourceMemory { pool: self.pool.clone(), slot })
    }
}
impl SourceMemory {
    pub fn max_bytes(&self) -> usize {
        MAX_SOURCE_BYTES
    }
    /// Reserves one complete window. An existing window or queued request for
    /// this source returns ReleaseRequired without entering the global queue.
    /// New sources wait FIFO and return Busy only when the deadline expires.
    pub async fn acquire(&self, bytes: usize, deadline: Instant) -> Result<SourceAdmission, Error> {
        if bytes == 0 || bytes > MAX_SOURCE_BYTES {
            return Err(Error::Invalid("source window must contain 1 byte through 12 MiB".into()));
        }
        let source = match self.slot.window.try_acquire(MAX_SOURCE_BYTES) {
            Ok(permit) => permit,
            Err(Error::Busy) => return Ok(SourceAdmission::ReleaseRequired),
            Err(error) => return Err(error),
        };
        let global = self.pool.budget.acquire_until(bytes, deadline).await?;
        Ok(SourceAdmission::Granted(SourceWindow { global, _source: source, _slot: self.slot.clone() }))
    }
}
impl SourceWindow {
    pub fn bytes(&self) -> usize {
        self.global.bytes()
    }
    pub fn flush_threshold(&self) -> usize {
        FLUSH_BYTES.min(self.bytes())
    }
}
impl Drop for SourceSlot {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.upgrade()
            && let Ok(mut sources) = pool.sources.lock()
            && sources.get(&self.key).is_some_and(|slot| std::ptr::eq(slot.as_ptr(), self))
        {
            // A replacement may have been installed while this destructor
            // waited for the lock. Never remove that replacement's entry.
            sources.remove(&self.key);
        }
    }
}

fn registry_unavailable() -> Error {
    Error::Unavailable("source memory registry unavailable".into())
}
