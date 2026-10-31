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
}

/// One kind of violation at one place, with its latest occurrence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    /// `blocking`: work that blocks its thread on a thread that must not
    /// block; `slow-poll`: a task kept its thread too long at once;
    /// `held-up`: a runtime's tasks waited too long for a thread.
    pub kind: String,
    /// Where: the source location, the task, or the runtime.
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
