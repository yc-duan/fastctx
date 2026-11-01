//! Allocation-free database measurements: the database's own, which drills
//! read through `Statistics`, and the process's readings beside them.

use crate::readings::Readings;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

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
    pub readings: Readings,
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

/// The most an operation may take on the writer.
const BUDGET: Duration = Duration::from_millis(2);
/// An operation slower than this is slow outright.
const SLOW: Duration = Duration::from_millis(20);

impl Metrics {
    pub fn new(readings: Readings) -> Self {
        Self {
            queued: AtomicUsize::new(0),
            operations: AtomicU64::new(0),
            commits: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            slow_operations: AtomicU64::new(0),
            writes_over_budget: AtomicU64::new(0),
            database_bytes: AtomicU64::new(0),
            wal_bytes: AtomicU64::new(0),
            write_micros: Histogram::default(),
            commit_micros: Histogram::default(),
            read_micros: Histogram::default(),
            readings,
        }
    }

    /// One operation's SQL on the writer, and whether it failed.
    pub fn operation(&self, took: Duration, failed: bool) {
        self.write_micros.record(took);
        self.readings.write.record(took);
        self.operations.fetch_add(1, Ordering::Relaxed);
        self.readings.operations.add(1);
        if failed {
            self.failures.fetch_add(1, Ordering::Relaxed);
            self.readings.failures.add(1);
        }
        if took > BUDGET {
            self.writes_over_budget.fetch_add(1, Ordering::Relaxed);
            self.readings.over_budget.add(1);
        }
        if took > SLOW {
            self.slow_operations.fetch_add(1, Ordering::Relaxed);
            self.readings.slow.add(1);
        }
    }

    pub fn commit(&self, took: Duration) {
        self.commit_micros.record(took);
        self.readings.commit.record(took);
        self.commits.fetch_add(1, Ordering::Relaxed);
        self.readings.commits.add(1);
    }

    pub fn read(&self, took: Duration) {
        self.read_micros.record(took);
        self.readings.read.record(took);
    }

    pub fn read_wait(&self, took: Duration) {
        self.readings.read_wait.record(took);
    }

    /// Operations waiting for the writer, by `delta`.
    pub fn queue(&self, delta: isize) {
        if delta >= 0 {
            self.queued.fetch_add(delta.unsigned_abs(), Ordering::Relaxed);
        } else {
            self.queued.fetch_sub(delta.unsigned_abs(), Ordering::Relaxed);
        }
        self.readings.queued.add(delta as i64);
    }

    pub fn sizes(&self, database: u64, log: u64) {
        self.database_bytes.store(database, Ordering::Relaxed);
        self.wal_bytes.store(log, Ordering::Relaxed);
        self.readings.size.set(i64::try_from(database).unwrap_or(i64::MAX));
        self.readings.log_size.set(i64::try_from(log).unwrap_or(i64::MAX));
    }

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
