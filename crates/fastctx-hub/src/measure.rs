//! Allocation-free duration histograms. Buckets report upper bounds in
//! microseconds; the maximum retains the exact largest observation. They are
//! finer than the platform's readings, whose smallest bucket is 50 µs, and
//! keep each thread's CPU time beside the wall time: the hub scale drill
//! compares views and presence costing a few tens of microseconds across
//! network sizes, and locks held for a few (`tests/hub_scale`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const LIMITS: [u64; 17] =
    [1, 5, 10, 25, 50, 100, 250, 500, 1_000, 2_000, 5_000, 10_000, 50_000, 100_000, 1_000_000, 10_000_000, u64::MAX];

pub(crate) struct Histogram {
    buckets: [AtomicU64; LIMITS.len()],
    maximum: AtomicU64,
    total: AtomicU64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            maximum: AtomicU64::new(0),
            total: AtomicU64::new(0),
        }
    }
}

impl Histogram {
    pub(crate) fn record(&self, duration: Duration) {
        let micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        let bucket = LIMITS.iter().position(|limit| micros <= *limit).unwrap_or(LIMITS.len() - 1);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.maximum.fetch_max(micros, Ordering::Relaxed);
        self.total.fetch_add(micros, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> serde_json::Value {
        let buckets: Vec<_> = self.buckets.iter().map(|n| n.load(Ordering::Relaxed)).collect();
        let count: u64 = buckets.iter().sum();
        let percentile = |percent: u64| {
            let mut accumulated = 0;
            let target = count.saturating_mul(percent).div_ceil(100);
            buckets
                .iter()
                .zip(LIMITS)
                .find_map(|(n, limit)| {
                    accumulated += n;
                    (accumulated >= target).then_some(limit)
                })
                .unwrap_or(0)
        };
        serde_json::json!({ "count": count, "p50_us": if count == 0 { 0 } else { percentile(50) }, "p99_us": if count == 0 { 0 } else { percentile(99) }, "mean_us": self.total.load(Ordering::Relaxed).checked_div(count).unwrap_or(0), "max_us": self.maximum.load(Ordering::Relaxed) })
    }
}

pub(crate) struct Timer<'a> {
    began: std::time::Instant,
    histogram: &'a Histogram,
}
impl Histogram {
    pub(crate) fn time(&self) -> Timer<'_> {
        Timer { began: std::time::Instant::now(), histogram: self }
    }
}
impl Drop for Timer<'_> {
    fn drop(&mut self) {
        self.histogram.record(self.began.elapsed());
    }
}

/// Synchronous operation timing. Wall time includes contention and scheduler
/// pauses; thread CPU time separates computational growth from CPU throttling.
#[derive(Default)]
pub(crate) struct Timing {
    wall: Histogram,
    cpu: Histogram,
}
impl Timing {
    pub(crate) fn time(&self) -> Timed<'_> {
        Timed { _wall: self.wall.time(), cpu: &self.cpu, began: thread_cpu() }
    }
    pub(crate) fn snapshot(&self) -> serde_json::Value {
        let mut value = self.wall.snapshot();
        value["cpu"] = self.cpu.snapshot();
        value
    }
}
pub(crate) struct Timed<'a> {
    _wall: Timer<'a>,
    cpu: &'a Histogram,
    began: Option<Duration>,
}
impl Drop for Timed<'_> {
    fn drop(&mut self) {
        if let Some(elapsed) = self.began.zip(thread_cpu()).and_then(|(before, after)| after.checked_sub(before)) {
            self.cpu.record(elapsed);
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn thread_cpu() -> Option<std::time::Duration> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // The pointer is valid for the duration of clock_gettime.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0 {
        return None;
    }
    Some(std::time::Duration::new(u64::try_from(time.tv_sec).ok()?, u32::try_from(time.tv_nsec).ok()?))
}
#[cfg(not(target_os = "linux"))]
pub(crate) fn thread_cpu() -> Option<std::time::Duration> {
    None
}
