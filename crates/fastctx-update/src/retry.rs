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
        self.postpone_from(now, Duration::from_secs(3600));
    }

    /// Reserves the next try, starting at `first` and doubling to one day.
    pub fn postpone_from(&mut self, now: Timestamp, first: Duration) {
        self.attempts = self.attempts.saturating_add(1);
        let seconds = (first.as_secs() << self.attempts.saturating_sub(1).min(5)).min(86400);
        self.next = now.checked_add(Duration::from_secs(seconds)).ok();
    }
}
