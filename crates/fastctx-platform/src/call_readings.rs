//! The latest call census. Heartbeats read atomics, never the call registry.

use fastctx_types::call_diagnostics::{Ages, Summary};
use std::sync::atomic::{AtomicU64, Ordering};

static REVISION: AtomicU64 = AtomicU64::new(0);
static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
static VALUES: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

pub fn set(summary: Summary) {
    static GAUGES: std::sync::LazyLock<[&'static crate::readings::Gauge; 32]> = std::sync::LazyLock::new(|| {
        [
            crate::readings::gauge("calls.subscriptions.count"),
            crate::readings::gauge("calls.subscriptions.oldest_seconds"),
            crate::readings::gauge("calls.subscriptions.overdue"),
            crate::readings::gauge("calls.subscriptions.under_60s"),
            crate::readings::gauge("calls.subscriptions.60s_to_5m"),
            crate::readings::gauge("calls.subscriptions.5m_to_1h"),
            crate::readings::gauge("calls.subscriptions.1h_to_1d"),
            crate::readings::gauge("calls.subscriptions.at_least_1d"),
            crate::readings::gauge("calls.requests.count"),
            crate::readings::gauge("calls.requests.oldest_seconds"),
            crate::readings::gauge("calls.requests.overdue"),
            crate::readings::gauge("calls.requests.under_60s"),
            crate::readings::gauge("calls.requests.60s_to_5m"),
            crate::readings::gauge("calls.requests.5m_to_1h"),
            crate::readings::gauge("calls.requests.1h_to_1d"),
            crate::readings::gauge("calls.requests.at_least_1d"),
            crate::readings::gauge("calls.operations.count"),
            crate::readings::gauge("calls.operations.oldest_seconds"),
            crate::readings::gauge("calls.operations.overdue"),
            crate::readings::gauge("calls.operations.under_60s"),
            crate::readings::gauge("calls.operations.60s_to_5m"),
            crate::readings::gauge("calls.operations.5m_to_1h"),
            crate::readings::gauge("calls.operations.1h_to_1d"),
            crate::readings::gauge("calls.operations.at_least_1d"),
            crate::readings::gauge("calls.unknown.count"),
            crate::readings::gauge("calls.unknown.oldest_seconds"),
            crate::readings::gauge("calls.unknown.overdue"),
            crate::readings::gauge("calls.unknown.under_60s"),
            crate::readings::gauge("calls.unknown.60s_to_5m"),
            crate::readings::gauge("calls.unknown.5m_to_1h"),
            crate::readings::gauge("calls.unknown.1h_to_1d"),
            crate::readings::gauge("calls.unknown.at_least_1d"),
        ]
    });
    let _writing = WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    REVISION.fetch_add(1, Ordering::SeqCst);
    for (index, (_, ages)) in summary.groups().into_iter().enumerate() {
        let values = [
            ages.count,
            ages.oldest_seconds,
            ages.overdue,
            ages.buckets[0],
            ages.buckets[1],
            ages.buckets[2],
            ages.buckets[3],
            ages.buckets[4],
        ];
        for (offset, value) in values.into_iter().enumerate() {
            VALUES[index * 8 + offset].store(value, Ordering::SeqCst);
            GAUGES[index * 8 + offset].set(value.min(i64::MAX as u64) as i64);
        }
    }
    REVISION.fetch_add(1, Ordering::SeqCst);
}

/// A heartbeat racing a census omits it instead of waiting or mixing two
/// samples. These values never determine admission or a stuck-work alert.
pub fn get() -> Option<Summary> {
    let revision = REVISION.load(Ordering::SeqCst);
    if revision == 0 || !revision.is_multiple_of(2) {
        return None;
    }
    let age = |index: usize| {
        let values: [u64; 8] = std::array::from_fn(|offset| VALUES[index * 8 + offset].load(Ordering::SeqCst));
        Ages {
            count: values[0],
            oldest_seconds: values[1],
            overdue: values[2],
            buckets: values[3..].try_into().expect("five age buckets"),
        }
    };
    let summary = Summary { subscriptions: age(0), requests: age(1), operations: age(2), unknown: age(3) };
    (REVISION.load(Ordering::SeqCst) == revision).then_some(summary)
}
