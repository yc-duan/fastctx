//! Allocation-free database measurements, independent of a service's reporter.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Default)]
pub(super) struct Metrics {
    pub queued: AtomicUsize,
    pub operations: AtomicU64,
    pub commits: AtomicU64,
    pub failures: AtomicU64,
    pub slow_operations: AtomicU64,
    pub writes_over_budget: AtomicU64,
    pub database_bytes: AtomicU64,
    pub wal_bytes: AtomicU64,
    pub write_micros: Histogram,
    pub commit_micros: Histogram,
    pub read_micros: Histogram,
}

#[derive(Default)]
pub(super) struct Histogram([AtomicU64; 32]);
impl Histogram {
    pub fn record(&self, elapsed: Duration) {
        let micros = elapsed.as_micros().min(u32::MAX as u128) as u32;
        let slot = (32 - micros.leading_zeros()).min(31) as usize;
        self.0[slot].fetch_add(1, Ordering::Relaxed);
    }
    fn snapshot(&self) -> [u64; 32] {
        std::array::from_fn(|i| self.0[i].load(Ordering::Relaxed))
    }
}

/// Histogram bucket zero contains zero microseconds; bucket n contains values
/// below 2^n microseconds. The final bucket also contains larger observations.
#[derive(Clone, Debug)]
pub struct Statistics {
    /// The database did not open; zero work counters do not indicate health.
    pub unavailable: Option<String>,
    pub queued: usize,
    pub operations: u64,
    pub commits: u64,
    pub failures: u64,
    pub slow_operations: u64,
    /// SQL operations exceeding the two-millisecond target, excluding FULL commit.
    pub writes_over_budget: u64,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub write_micros: [u64; 32],
    pub commit_micros: [u64; 32],
    pub read_micros: [u64; 32],
}
impl Metrics {
    pub fn snapshot(&self) -> Statistics {
        Statistics {
            unavailable: None,
            queued: self.queued.load(Ordering::Relaxed),
            operations: self.operations.load(Ordering::Relaxed),
            commits: self.commits.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            slow_operations: self.slow_operations.load(Ordering::Relaxed),
            writes_over_budget: self.writes_over_budget.load(Ordering::Relaxed),
            database_bytes: self.database_bytes.load(Ordering::Relaxed),
            wal_bytes: self.wal_bytes.load(Ordering::Relaxed),
            write_micros: self.write_micros.snapshot(),
            commit_micros: self.commit_micros.snapshot(),
            read_micros: self.read_micros.snapshot(),
        }
    }
}
