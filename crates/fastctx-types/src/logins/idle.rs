//! Compacting a conversation once it has been idle a while (Issue #57): set
//! for each login by each account that uses it, apart from every other
//! account's, and off until that account turns it on. A conversation follows
//! the setting of its login for the account it works for in each part the
//! user did not set for it alone (`About::idle_compaction`).

use serde::{Deserialize, Serialize};

/// What a setting never changed holds, once turned on.
pub const AFTER_MINUTES: u32 = 50;
pub const MIN_TOKENS: u64 = 100_000;

/// The bounds a setting is kept within.
pub const AFTER_MINUTES_LEAST: u32 = 5;
pub const AFTER_MINUTES_MOST: u32 = 240;
pub const MIN_TOKENS_MOST: u64 = 2_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdleCompaction {
    pub on: bool,
    /// How long a conversation is idle before it is compacted.
    pub after_minutes: u32,
    /// The least context, in tokens, a conversation compacted holds.
    pub min_tokens: u64,
}

impl Default for IdleCompaction {
    fn default() -> Self {
        Self { on: false, after_minutes: AFTER_MINUTES, min_tokens: MIN_TOKENS }
    }
}

impl IdleCompaction {
    /// Why it cannot be kept, where it cannot.
    pub fn problem(&self) -> Option<String> {
        minutes_problem(self.after_minutes).or_else(|| tokens_problem(self.min_tokens))
    }
}

fn minutes_problem(minutes: u32) -> Option<String> {
    (!(AFTER_MINUTES_LEAST..=AFTER_MINUTES_MOST).contains(&minutes))
        .then(|| format!("the minutes idle are from {AFTER_MINUTES_LEAST} to {AFTER_MINUTES_MOST}, not {minutes}"))
}

fn tokens_problem(tokens: u64) -> Option<String> {
    (tokens > MIN_TOKENS_MOST).then(|| format!("the least context is at most {MIN_TOKENS_MOST} tokens, not {tokens}"))
}

/// What the user set for one conversation alone: each part none follows the
/// setting of its login for the account it works for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdleChoice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_tokens: Option<u64>,
}

impl IdleChoice {
    /// Following its login in every part.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The setting it compacts by: what was set for it alone over its
    /// login's.
    pub fn over(&self, login: IdleCompaction) -> IdleCompaction {
        IdleCompaction {
            on: self.on.unwrap_or(login.on),
            after_minutes: self.after_minutes.unwrap_or(login.after_minutes),
            min_tokens: self.min_tokens.unwrap_or(login.min_tokens),
        }
    }

    /// Why it cannot be kept, where it cannot.
    pub fn problem(&self) -> Option<String> {
        self.after_minutes.and_then(minutes_problem).or_else(|| self.min_tokens.and_then(tokens_problem))
    }
}
