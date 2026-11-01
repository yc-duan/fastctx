//! This process's health (`fastctx_types::health`), kept in atomics as it
//! changes, so the heartbeat reads it on the lines' thread without waiting
//! on anything: how long each runtime's work waited for a thread (from
//! `crate::task::watch`), what is stuck, the rules broken (`crate::conduct`),
//! and how much of each registered bound was used.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fastctx_types::health::{BUSY_MS, Health, Migrating, NEAR, Near, WINDOW_SECS};

const WINDOW: u64 = WINDOW_SECS * 1000;

/// Milliseconds since this process first asked, from one; zero means never.
fn now() -> u64 {
    static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    STARTED.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// How one runtime keeps up with its work, as its watch sees it.
pub(crate) struct Lag {
    /// Since when it is behind, while it is; zero when it is not.
    behind: AtomicU64,
    /// The longest it was behind lately, and when that ended.
    worst: AtomicU64,
    worst_at: AtomicU64,
}

/// The most runtimes watched; one past them is not counted.
const RUNTIMES: usize = 8;
static LAGS: [Lag; RUNTIMES] = [const { Lag::new() }; RUNTIMES];
static CLAIMED: AtomicUsize = AtomicUsize::new(0);

impl Lag {
    const fn new() -> Self {
        Self { behind: AtomicU64::new(0), worst: AtomicU64::new(0), worst_at: AtomicU64::new(0) }
    }

    /// The account of one more runtime.
    pub(crate) fn claim() -> Option<&'static Lag> {
        LAGS.get(CLAIMED.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) fn fell_behind(&self, since: Instant) {
        let ago = since.elapsed().as_millis() as u64;
        self.behind.store(now().saturating_sub(ago).max(1), Ordering::Relaxed);
    }

    pub(crate) fn caught_up(&self, waited: Duration) {
        self.behind.store(0, Ordering::Relaxed);
        let (waited, at) = (waited.as_millis() as u64, now());
        let stale = at.saturating_sub(self.worst_at.load(Ordering::Relaxed)) > WINDOW;
        if stale || waited >= self.worst.load(Ordering::Relaxed) {
            self.worst.store(waited, Ordering::Relaxed);
            self.worst_at.store(at, Ordering::Relaxed);
        }
    }

    /// The longest its work waited within the window, the wait going on now
    /// included.
    fn waited(&self, at: u64) -> u64 {
        let behind = self.behind.load(Ordering::Relaxed);
        let now_waiting = if behind == 0 { 0 } else { at.saturating_sub(behind) };
        let lately = self.worst_at.load(Ordering::Relaxed);
        let recent =
            if lately != 0 && at.saturating_sub(lately) <= WINDOW { self.worst.load(Ordering::Relaxed) } else { 0 };
        now_waiting.max(recent)
    }
}

static STUCK: AtomicU32 = AtomicU32::new(0);

static DAMAGED: RwLock<Option<&'static str>> = RwLock::new(None);

/// Notes that the store `name` of this process's records was found damaged;
/// the health tells of it from then on.
pub fn damaged(name: &'static str) {
    *DAMAGED.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(name);
}

/// How many of this process's tasks are stuck now.
pub fn set_stuck(stuck: usize) {
    STUCK.store(u32::try_from(stuck).unwrap_or(u32::MAX), Ordering::Relaxed);
}

/// A bound kept to, and the most of it used lately.
pub struct Bound {
    name: &'static str,
    /// In hundredths.
    peak: AtomicU64,
    peak_at: AtomicU64,
}

impl Bound {
    /// Notes that `used` of `limit` is in use now.
    pub fn note(&self, used: usize, limit: usize) {
        if limit == 0 {
            return;
        }
        let hundredths = (used.saturating_mul(100) / limit).min(100) as u64;
        let at = now();
        let stale = at.saturating_sub(self.peak_at.load(Ordering::Relaxed)) > WINDOW;
        if stale || hundredths >= self.peak.load(Ordering::Relaxed) {
            self.peak.store(hundredths, Ordering::Relaxed);
            self.peak_at.store(at, Ordering::Relaxed);
        }
    }

    /// The most used within the window.
    fn recent(&self, at: u64) -> u64 {
        let lately = self.peak_at.load(Ordering::Relaxed);
        if lately != 0 && at.saturating_sub(lately) <= WINDOW { self.peak.load(Ordering::Relaxed) } else { 0 }
    }
}

static BOUNDS: RwLock<Vec<&'static Bound>> = RwLock::new(Vec::new());

/// The bound named `name`, made the first time it is asked for. Whoever
/// notes it often keeps what this returns.
pub fn bound(name: &'static str) -> &'static Bound {
    if let Some(bound) =
        BOUNDS.read().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().find(|bound| bound.name == name)
    {
        return bound;
    }
    let mut bounds = BOUNDS.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(bound) = bounds.iter().find(|bound| bound.name == name) {
        return bound;
    }
    let bound: &'static Bound =
        Box::leak(Box::new(Bound { name, peak: AtomicU64::new(0), peak_at: AtomicU64::new(0) }));
    bounds.push(bound);
    bound
}

/// The health to tell now.
pub fn summary() -> Health {
    let at = now();
    let claimed = CLAIMED.load(Ordering::Relaxed).min(RUNTIMES);
    let held_up_ms = LAGS[..claimed].iter().map(|lag| lag.waited(at)).max().unwrap_or(0);
    let near = BOUNDS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|bound| (bound.name, bound.recent(at)))
        .max_by_key(|(_, used)| *used)
        .filter(|(_, used)| *used >= u64::from(NEAR))
        .map(|(bound, used)| Near { bound: bound.to_string(), used: used as u8 });
    Health {
        busy: held_up_ms >= BUSY_MS,
        held_up_ms,
        stuck: STUCK.load(Ordering::Relaxed),
        violations: crate::conduct::total(),
        near,
        damaged: DAMAGED.read().unwrap_or_else(|poisoned| poisoned.into_inner()).map(str::to_string),
        migrating: migrating(),
    }
}

/// How the move of earlier records goes, from what the node's migration reads
/// (`migration.*`): told while any are left, failed or set aside.
fn migrating() -> Option<Migrating> {
    let read = |name| crate::readings::gauge(name).get().max(0) as u64;
    let migrating = Migrating {
        remaining: read("migration.discovery_remaining"),
        failed: read("migration.failed_families").min(u64::from(u32::MAX)) as u32,
        damaged: read("migration.damaged_pending").min(u64::from(u32::MAX)) as u32,
    };
    (migrating != Migrating::default()).then_some(migrating)
}
