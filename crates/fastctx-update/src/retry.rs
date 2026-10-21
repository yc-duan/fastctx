//! Durable delays between repeated attempts at the same update.

use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Retry {
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub next: Option<Timestamp>,
}

impl Retry {
    pub fn ready(&self, now: Timestamp) -> bool {
        self.next.is_none_or(|next| now >= next)
    }

    /// Reserves the next try, starting at one hour and growing to one day.
    pub fn postpone(&mut self, now: Timestamp) {
        self.attempts = self.attempts.saturating_add(1);
        let seconds = (3600u64 << self.attempts.saturating_sub(1).min(5)).min(86400);
        self.next = now.checked_add(Duration::from_secs(seconds)).ok();
    }
}
