//! A machine's health in a few numbers, carried by its heartbeat
//! (`specs/world-engine.md`, 量与告警): whether it is busy, how long its work
//! waited lately, what is stuck, how often a rule was broken, the bound
//! nearest its limit, a store of its records found damaged, and records of an
//! earlier version still moving in. The hub shows
//! a machine as busy or stuck beside online from it, and gathers the
//! network's health.

use serde::{Deserialize, Serialize};

/// How long ago what a machine tells of its health counts: its numbers
/// are the worst within this window.
pub const WINDOW_SECS: u64 = 60;

/// How long work must have waited for a thread within the window for the
/// machine to be busy.
pub const BUSY_MS: u64 = 1000;

/// How much of a bound, in hundredths, is near its limit.
pub const NEAR: u8 = 90;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// Its work waited past `BUSY_MS` for a thread within the window.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub busy: bool,
    /// The longest its work waited for a thread within the window, in
    /// milliseconds.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub held_up_ms: u64,
    /// How many of its tasks are stuck now.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub stuck: u32,
    /// How many times a rule was broken since it started.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub violations: u64,
    /// The bound used the most within the window, once it reached `NEAR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near: Option<Near>,
    /// A store of its records found damaged since it started, by name
    /// (`storage.node`): some of what it kept may be lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damaged: Option<String>,
    /// Records of an earlier version still moving into its databases, while
    /// any are left, failed or set aside (`specs/world-engine.md`, 兼容与迁移).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrating: Option<Migrating>,
    /// The latest census of admitted calls. Ages alone never imply stuck work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<crate::call_diagnostics::Summary>,
}

/// How the move of a machine's earlier records goes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Migrating {
    /// Conversations and jobs whose earlier records are still to be looked at.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub remaining: u64,
    /// Kinds of records that failed to move and wait to be tried again.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub failed: u32,
    /// Earlier files that could not be read, set aside with their reason.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub damaged: u32,
}

impl Health {
    /// How the machine's own work goes, by these numbers: something stuck
    /// says more than being busy.
    pub fn strain(&self) -> Option<Strain> {
        if self.stuck > 0 {
            Some(Strain::Stuck)
        } else if self.busy {
            Some(Strain::Busy)
        } else {
            None
        }
    }
}

/// How a machine's own work goes, beside its being online, as its hub tells
/// those who see it: none while its work goes as it should, or a machine
/// does not tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strain {
    /// Its work waited for a thread past `BUSY_MS` within the window.
    Busy,
    /// Some of its work has not gone on for a while.
    Stuck,
}

impl crate::tolerant::Tolerant for Strain {
    const ALL: &'static [Self] = &[Strain::Busy, Strain::Stuck];
    /// A strain a later release tells is read as busy: something is off,
    /// and no more is claimed.
    const UNKNOWN: Self = Strain::Busy;
}

/// A bound near its limit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Near {
    pub bound: String,
    /// How much of it was used at most, in hundredths.
    pub used: u8,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}
