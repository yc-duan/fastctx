//! What a measure reports: p50, p99 and the most of its samples, how many
//! there were and how many failed, against its target.

use std::time::Duration;

/// Durations measured, failures apart.
#[derive(Default)]
pub(crate) struct Samples {
    took: Vec<Duration>,
    pub(crate) failed: usize,
}

impl Samples {
    pub(crate) fn push(&mut self, took: Duration) {
        self.took.push(took);
    }

    fn quantile(sorted: &[Duration], quantile: f64) -> Duration {
        if sorted.is_empty() {
            return Duration::ZERO;
        }
        let at = ((sorted.len() as f64 - 1.0) * quantile).round() as usize;
        sorted[at.min(sorted.len() - 1)]
    }

    /// The 99th percentile, in milliseconds.
    pub(crate) fn p99_ms(&self) -> f64 {
        let mut sorted = self.took.clone();
        sorted.sort();
        Self::quantile(&sorted, 0.99).as_secs_f64() * 1000.0
    }

    /// Prints the measure `measure` of `row` against `target` (ms at p99);
    /// whether it was met.
    pub(crate) fn report(mut self, row: &str, measure: &str, target_ms: Option<f64>, extra: serde_json::Value) -> bool {
        self.took.sort();
        let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
        let (p50, p99, most) = (Self::quantile(&self.took, 0.5), Self::quantile(&self.took, 0.99), self.took.last().copied().unwrap_or_default());
        let met = self.failed == 0 && target_ms.is_none_or(|target| ms(p99) <= target);
        println!(
            "{}",
            serde_json::json!({
                "row": row, "measure": measure, "samples": self.took.len(), "failed": self.failed,
                "p50_ms": round(ms(p50)), "p99_ms": round(ms(p99)), "max_ms": round(ms(most)),
                "target_p99_ms": target_ms, "met": met, "more": extra,
            })
        );
        met
    }
}

/// Prints a rate measured once: `amount` of `unit` in `took`, against a
/// least rate `target`; whether it was met.
pub(crate) fn rate(row: &str, measure: &str, amount: f64, unit: &str, took: Duration, target: f64, extra: serde_json::Value) -> bool {
    let per_second = amount / took.as_secs_f64().max(1e-9);
    let met = per_second >= target;
    println!(
        "{}",
        serde_json::json!({
            "row": row, "measure": measure, "amount": amount, "unit": unit, "seconds": round(took.as_secs_f64()),
            "per_second": round(per_second), "target_per_second": target, "met": met, "more": extra,
        })
    );
    met
}

pub(crate) fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
