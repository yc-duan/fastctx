use super::{Cause, Counter, Gauge, Latency, Metrics};
use fastctx_platform::readings;
use std::{sync::OnceLock, time::Duration};

static COUNTERS: [[OnceLock<&'static readings::Counter>; Cause::COUNT]; Counter::COUNT] =
    [const { [const { OnceLock::new() }; Cause::COUNT] }; Counter::COUNT];
static GAUGES: [OnceLock<&'static readings::Gauge>; Gauge::COUNT] = [const { OnceLock::new() }; Gauge::COUNT];
static LATENCIES: [OnceLock<&'static readings::Timing>; Latency::COUNT] = [const { OnceLock::new() }; Latency::COUNT];

/// Registers only fixed enum combinations; initialized hot paths use atomics.
#[derive(Clone, Copy, Default)]
pub struct PlatformMetrics;
impl Metrics for PlatformMetrics {
    fn add(&self, counter: Counter, cause: Cause, amount: u64) {
        COUNTERS[counter as usize][cause as usize]
            .get_or_init(|| {
                readings::counter(Box::leak(format!("{}.{}", counter.label(), cause.label()).into_boxed_str()))
            })
            .add(amount);
    }
    fn set(&self, gauge: Gauge, value: u64) {
        GAUGES[gauge as usize]
            .get_or_init(|| readings::gauge(gauge.label()))
            .set(i64::try_from(value).unwrap_or(i64::MAX));
    }
    fn elapsed(&self, latency: Latency, nanos: u64) {
        LATENCIES[latency as usize]
            .get_or_init(|| readings::timing(latency.label()))
            .record(Duration::from_nanos(nanos));
    }
}
