//! The process's measures of itself (`specs/world-engine.md`, 量与告警):
//! counts, current values, and timings in fixed buckets, kept in atomics so
//! that a hot path neither allocates nor waits.
//!
//! A measure is registered once by name and lives as long as the process;
//! a caller on a hot path keeps the reference it got in a static.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use fastctx_types::readings::{Count, Level, Readings, Timing as TimingReading};

pub struct Counter(AtomicU64);

impl Counter {
    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

pub struct Gauge(AtomicI64);

impl Gauge {
    pub fn set(&self, value: i64) {
        self.0.store(value, Ordering::Relaxed);
    }

    pub fn add(&self, delta: i64) {
        self.0.fetch_add(delta, Ordering::Relaxed);
    }

    pub fn get(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Upper bounds of the buckets, in microseconds; the last takes the rest.
const BOUNDS_US: [u64; 19] = [
    50,
    100,
    250,
    500,
    1_000,
    2_500,
    5_000,
    10_000,
    25_000,
    50_000,
    100_000,
    250_000,
    500_000,
    1_000_000,
    2_500_000,
    5_000_000,
    10_000_000,
    30_000_000,
    u64::MAX,
];

pub struct Timing {
    buckets: [AtomicU64; BOUNDS_US.len()],
    count: AtomicU64,
    sum_us: AtomicU64,
    max_us: AtomicU64,
}

impl Timing {
    fn new() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            count: AtomicU64::new(0),
            sum_us: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
        }
    }

    pub fn record(&self, took: Duration) {
        let us = u64::try_from(took.as_micros()).unwrap_or(u64::MAX);
        let bucket = BOUNDS_US.iter().position(|bound| us <= *bound).unwrap_or(BOUNDS_US.len() - 1);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(us, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
    }

    fn reading(&self, name: &str) -> TimingReading {
        let counts: Vec<u64> = self.buckets.iter().map(|bucket| bucket.load(Ordering::Relaxed)).collect();
        let count: u64 = counts.iter().sum();
        let max_us = self.max_us.load(Ordering::Relaxed);
        let at = |share: f64| -> u64 {
            if count == 0 {
                return 0;
            }
            let wanted = ((count as f64) * share).ceil().max(1.0) as u64;
            let mut seen = 0;
            for (bucket, n) in counts.iter().enumerate() {
                seen += n;
                if seen >= wanted {
                    return BOUNDS_US[bucket].min(max_us);
                }
            }
            max_us
        };
        TimingReading {
            name: name.to_string(),
            count,
            p50_us: at(0.5),
            p99_us: at(0.99),
            max_us,
            mean_us: self.sum_us.load(Ordering::Relaxed).checked_div(count).unwrap_or(0),
        }
    }
}

enum Measure {
    Counter(&'static Counter),
    Gauge(&'static Gauge),
    Timing(&'static Timing),
}

static MEASURES: LazyLock<Mutex<BTreeMap<&'static str, Measure>>> = LazyLock::new(Default::default);

fn measures() -> std::sync::MutexGuard<'static, BTreeMap<&'static str, Measure>> {
    MEASURES.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The counter named `name`. A name registered as another kind of measure
/// is a mistake in the code, so it stops the process there.
pub fn counter(name: &'static str) -> &'static Counter {
    let mut measures = measures();
    match measures.entry(name).or_insert_with(|| Measure::Counter(Box::leak(Box::new(Counter(AtomicU64::new(0)))))) {
        Measure::Counter(counter) => counter,
        _ => panic!("the measure {name} is not a counter"),
    }
}

pub fn gauge(name: &'static str) -> &'static Gauge {
    let mut measures = measures();
    match measures.entry(name).or_insert_with(|| Measure::Gauge(Box::leak(Box::new(Gauge(AtomicI64::new(0)))))) {
        Measure::Gauge(gauge) => gauge,
        _ => panic!("the measure {name} is not a gauge"),
    }
}

pub fn timing(name: &'static str) -> &'static Timing {
    let mut measures = measures();
    match measures.entry(name).or_insert_with(|| Measure::Timing(Box::leak(Box::new(Timing::new())))) {
        Measure::Timing(timing) => timing,
        _ => panic!("the measure {name} is not a timing"),
    }
}

/// Every measure as it stands, with the violations so far.
pub fn snapshot() -> Readings {
    let mut readings = Readings::default();
    for (name, measure) in measures().iter() {
        match measure {
            Measure::Counter(counter) => readings.counts.push(Count { name: name.to_string(), value: counter.get() }),
            Measure::Gauge(gauge) => readings.levels.push(Level { name: name.to_string(), value: gauge.get() }),
            Measure::Timing(timing) => readings.timings.push(timing.reading(name)),
        }
    }
    readings.violations = crate::conduct::snapshot();
    readings.violations_total = crate::conduct::total();
    readings
}
