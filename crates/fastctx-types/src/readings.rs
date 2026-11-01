//! A process's measures of itself at one moment (`specs/world-engine.md`,
//! 量与告警): counts, current values, timings, and what it did against its
//! own rules.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Readings {
    #[serde(default)]
    pub counts: Vec<Count>,
    #[serde(default)]
    pub levels: Vec<Level>,
    #[serde(default)]
    pub timings: Vec<Timing>,
    #[serde(default)]
    pub violations: Vec<Violation>,
    /// Every violation since the process started, also those of places no
    /// longer listed.
    #[serde(default)]
    pub violations_total: u64,
    /// The upper bounds of the timings' buckets, in microseconds; the last
    /// takes the rest. Empty from a process that does not tell its buckets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bounds_us: Vec<u64>,
}

/// How many times something happened since the process started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Count {
    pub name: String,
    pub value: u64,
}

/// How much of something there is now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Level {
    pub name: String,
    pub value: i64,
}

/// How long something took, over every time it was timed: the median and
/// the 99th percentile are the upper bounds of the buckets they fall in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    pub name: String,
    pub count: u64,
    pub p50_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
    pub mean_us: u64,
    #[serde(default)]
    pub sum_us: u64,
    /// How many times fell in each bucket of `Readings::bounds_us`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<u64>,
}

/// One kind of violation at one place, with its latest occurrence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    /// `blocking`: work that blocks its thread on a thread that must not
    /// block; `slow-poll`: a task kept its thread too long at once;
    /// `held-up`: a runtime's tasks waited too long for a thread;
    /// `held-long`: a lock over many units was held too long.
    pub kind: String,
    /// Where: the source location, the task, the runtime, or the lock.
    pub place: String,
    pub count: u64,
    /// What the task or thread said it was doing the last time.
    #[serde(default)]
    pub doing: String,
    /// The longest it took, for the kinds that are timed.
    #[serde(default)]
    pub worst_ms: u64,
    pub last: jiff::Timestamp,
}

/// The upper bound of the bucket in which `share` of the times counted in
/// `buckets` falls, never past `max_us`, the longest of them.
pub fn percentile(bounds_us: &[u64], buckets: &[u64], max_us: u64, share: f64) -> u64 {
    let count: u64 = buckets.iter().sum();
    if count == 0 {
        return 0;
    }
    let wanted = ((count as f64) * share).ceil().max(1.0) as u64;
    let mut seen = 0;
    for (bucket, n) in buckets.iter().enumerate() {
        seen += n;
        if seen >= wanted {
            return bounds_us.get(bucket).copied().unwrap_or(u64::MAX).min(max_us);
        }
    }
    max_us
}

impl Timing {
    /// The times of this timing that came after `earlier`, an earlier
    /// reading of the same timing in the same process. Its longest is the
    /// upper bound of the last bucket those times fell in, or the longest
    /// ever when that is less.
    pub fn since(&self, earlier: Option<&Timing>, bounds_us: &[u64]) -> Timing {
        let buckets: Vec<u64> = match earlier {
            Some(earlier) if earlier.buckets.len() == self.buckets.len() => {
                self.buckets.iter().zip(&earlier.buckets).map(|(now, then)| now.saturating_sub(*then)).collect()
            }
            _ => self.buckets.clone(),
        };
        let count = buckets.iter().sum::<u64>();
        let sum_us = self.sum_us.saturating_sub(earlier.map_or(0, |earlier| earlier.sum_us));
        let max_us = buckets
            .iter()
            .rposition(|n| *n > 0)
            .map_or(0, |last| bounds_us.get(last).copied().unwrap_or(u64::MAX).min(self.max_us));
        Timing {
            name: self.name.clone(),
            count,
            p50_us: percentile(bounds_us, &buckets, max_us, 0.5),
            p99_us: percentile(bounds_us, &buckets, max_us, 0.99),
            max_us,
            mean_us: sum_us.checked_div(count).unwrap_or(0),
            sum_us,
            buckets,
        }
    }
}

impl Readings {
    /// What the process did between `earlier`, an earlier reading of the
    /// same process, and this one: counts and timings of that stretch alone,
    /// levels as they are now, violations counted since.
    pub fn since(&self, earlier: &Readings) -> Readings {
        let counts = self
            .counts
            .iter()
            .map(|count| {
                let then = earlier.counts.iter().find(|then| then.name == count.name).map_or(0, |then| then.value);
                Count { name: count.name.clone(), value: count.value.saturating_sub(then) }
            })
            .collect();
        let timings = self
            .timings
            .iter()
            .map(|timing| timing.since(earlier.timings.iter().find(|then| then.name == timing.name), &self.bounds_us))
            .collect();
        let violations = self
            .violations
            .iter()
            .filter_map(|violation| {
                let then = earlier
                    .violations
                    .iter()
                    .find(|then| then.kind == violation.kind && then.place == violation.place)
                    .map_or(0, |then| then.count);
                (violation.count > then).then(|| Violation { count: violation.count - then, ..violation.clone() })
            })
            .collect();
        Readings {
            counts,
            levels: self.levels.clone(),
            timings,
            violations,
            violations_total: self.violations_total.saturating_sub(earlier.violations_total),
            bounds_us: self.bounds_us.clone(),
        }
    }

    pub fn count(&self, name: &str) -> Option<u64> {
        self.counts.iter().find(|count| count.name == name).map(|count| count.value)
    }

    pub fn level(&self, name: &str) -> Option<i64> {
        self.levels.iter().find(|level| level.name == name).map(|level| level.value)
    }

    pub fn timing(&self, name: &str) -> Option<&Timing> {
        self.timings.iter().find(|timing| timing.name == name)
    }
}
