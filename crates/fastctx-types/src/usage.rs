//! What an agent's login was used for (`world-agent-logins.md`, 用量): for
//! each account its conversations worked for, the tokens of each kind by
//! the hour and, for Claude Code, what its own cost ledger said they cost;
//! what the current week went to, by model and by kind, in all and for each
//! account; the login's reset weeks over the year, with who used how much in
//! each; and the login's usage windows as they were read through the week.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::access::HubAccount;
use crate::agents::LimitWindow;

/// An amount in US dollars, in millionths of a dollar: what Claude Code's
/// cost ledger says, summed without rounding, and the weekly allowance an
/// assistance gives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Usd(pub u64);

impl Usd {
    pub const ZERO: Usd = Usd(0);

    /// From a host's dollar figure; none for one that is not a finite,
    /// non-negative amount.
    pub fn from_dollars(dollars: f64) -> Option<Usd> {
        (dollars.is_finite() && dollars >= 0.0).then(|| Usd((dollars * 1_000_000.0).round() as u64))
    }

    pub fn saturating_add(self, other: Usd) -> Usd {
        Usd(self.0.saturating_add(other.0))
    }

    pub fn saturating_sub(self, other: Usd) -> Usd {
        Usd(self.0.saturating_sub(other.0))
    }
}

/// The tokens of one stretch of use, by kind. Input is what was neither read
/// from the prompt cache nor written to it; reasoning is Codex's, counted
/// apart from its other output; Claude Code's thinking is in its output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub input: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub output: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reasoning: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
            .saturating_add(self.output)
            .saturating_add(self.reasoning)
    }

    pub fn add(&mut self, other: &Tokens) {
        self.input = self.input.saturating_add(other.input);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.output = self.output.saturating_add(other.output);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// A login's use as one reader may see it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReport {
    /// The accounts its conversations worked for; `None` is the user of the
    /// machine itself, whose own sessions work for whoever is logged in
    /// there. One the login assists sees only itself.
    pub people: Vec<UsagePerson>,
    /// Each hour anyone here used it over the last year, oldest first.
    pub hours: Vec<UsageHour>,
    /// The week the login's own seven-day window is in, and what went on in
    /// it.
    pub week: UsageWeek,
    /// The login's usage windows as read through that week, oldest first;
    /// none to an account the login only assists, which does not see its
    /// account's windows.
    pub readings: Vec<UsageReading>,
    /// The weeks of the login's own seven-day window its readings recorded
    /// over the year, oldest first, the current week last, with what each
    /// person used in each; none where no reading had that window. Older
    /// nodes do not say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weeks: Vec<UsageSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsagePerson {
    pub account: Option<HubAccount>,
    /// What it used in the week, as its weekly allowance counts it.
    pub week: UsageSum,
    /// What it used over the last 24 hours, as its daily allowance counts
    /// it. Older nodes do not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<UsageSum>,
    /// What it used on Fable models in this week. Older nodes do not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fable: Option<UsageSum>,
    /// What its week went to. Older nodes do not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub went: Option<WeekUse>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSum {
    pub tokens: u64,
    /// None where the agent keeps no cost ledger (Codex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<Usd>,
}

impl UsageSum {
    pub fn add(&mut self, tokens: u64, usd: Option<Usd>) {
        self.tokens = self.tokens.saturating_add(tokens);
        if let Some(usd) = usd {
            self.usd = Some(self.usd.unwrap_or(Usd::ZERO).saturating_add(usd));
        }
    }
}

/// One hour's use for one person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageHour {
    /// The hour's start, or the weekly boundary that split it.
    pub at: Timestamp,
    /// Its place in `people`.
    pub person: u32,
    #[serde(flatten)]
    pub sum: UsageSum,
}

/// One week of the login's own seven-day window: a week back from a reset a
/// reading had, ended sooner where the next week began sooner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSpan {
    pub start: Timestamp,
    pub end: Timestamp,
    /// How much of the seven-day window the login's account had used by
    /// the week's highest reading, in tenths of a percent; none to an account
    /// the login only assists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_tenths: Option<u32>,
    /// What each person who used the login in it used, by their place in
    /// `people`.
    pub used: Vec<UsageShare>,
}

/// One person's use of a week.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageShare {
    /// Its place in `people`.
    pub person: u32,
    #[serde(flatten)]
    pub sum: UsageSum,
    /// Their part of what the account used of the week's window, in tenths
    /// of a percent: the window's use shared out by what each person used
    /// as Fastctx counted it, Claude Code's dollars or Codex's tokens at
    /// list-price weights. None where the week has no reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_tenths: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageWeek {
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
    /// Whether the week is the login's own seven-day window as last read;
    /// otherwise it is the seven days up to now.
    pub window: bool,
    #[serde(flatten)]
    pub went: WeekUse,
}

/// What a week's use went to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekUse {
    /// Each model it went to, the most first.
    pub models: Vec<ModelUse>,
    /// The tokens of each kind, and for Claude Code what each kind cost at
    /// its models' list prices, scaled to what the ledger says all cost.
    pub tokens: Tokens,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds_usd: Option<KindsUsd>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUse {
    pub model: String,
    #[serde(flatten)]
    pub sum: UsageSum,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindsUsd {
    pub input: Usd,
    pub cache_read: Usd,
    pub cache_write: Usd,
    pub output: Usd,
}

/// The login's usage windows as one reading had them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReading {
    pub at: Timestamp,
    pub windows: Vec<LimitWindow>,
}
