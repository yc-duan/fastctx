//! Compacting a conversation once it has been idle a while (Issue #57): set
//! for each login by each account that uses it, apart from every other
//! account's, and off until that account turns it on. A conversation follows
//! the setting of its login for the account it works for unless it was told
//! otherwise (`About::idle_compaction`).

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
        if !(AFTER_MINUTES_LEAST..=AFTER_MINUTES_MOST).contains(&self.after_minutes) {
            return Some(format!(
                "the minutes idle are from {AFTER_MINUTES_LEAST} to {AFTER_MINUTES_MOST}, not {}",
                self.after_minutes
            ));
        }
        if self.min_tokens > MIN_TOKENS_MOST {
            return Some(format!("the least context is at most {MIN_TOKENS_MOST} tokens, not {}", self.min_tokens));
        }
        None
    }
}
