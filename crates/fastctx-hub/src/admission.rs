//! The most connections the hub's port holds at once. Each costs its TLS
//! state and buffers, so the count follows the hub's memory; past it a new
//! connection is closed at once and the refusals are logged now and then,
//! rather than every connection taking memory without end.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A connection's place, held until it and anything upgraded from it ends.
pub(crate) type Held = Arc<OwnedSemaphorePermit>;

/// How often refusals are written down at most.
const TELL_EVERY: Duration = Duration::from_secs(60);

pub(crate) struct Admission {
    places: Arc<Semaphore>,
    refused: AtomicU64,
    told: Mutex<Option<Instant>>,
}

impl Admission {
    pub(crate) fn new() -> Self {
        Self { places: Arc::new(Semaphore::new(limit())), refused: AtomicU64::new(0), told: Mutex::new(None) }
    }

    /// A place for the connection from `peer`, or none when every place is
    /// taken: the caller closes it.
    pub(crate) fn admit(&self, log: &crate::log::Log, peer: SocketAddr) -> Option<Held> {
        if let Ok(place) = self.places.clone().try_acquire_owned() {
            return Some(Arc::new(place));
        }
        let refused = self.refused.fetch_add(1, Ordering::Relaxed) + 1;
        let mut told = self.told.lock().expect("never poisoned");
        if told.is_none_or(|at| at.elapsed() >= TELL_EVERY) {
            *told = Some(Instant::now());
            drop(told);
            log.line(format_args!(
                "{peer}: refused a connection: all {} places are taken ({refused} refused since the hub started)",
                limit()
            ));
        }
        None
    }
}

/// One place for every 64 KiB of the hub's memory, from 8,192 to 262,144:
/// ten thousand members with their control and data connections fit in the
/// smallest.
fn limit() -> usize {
    static LIMIT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        (fastctx_platform::memory::physical().unwrap_or(1 << 30) / (64 << 10)).clamp(8_192, 262_144) as usize
    })
}
