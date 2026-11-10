//! Retry timing for a parent history, independent of its live event stream.

use std::time::Duration;
use tokio::time::Instant;

const FIRST: Duration = Duration::from_secs(30);
const MOST: Duration = Duration::from_secs(10 * 60);

pub(super) struct History {
    incomplete: bool,
    again: Option<Instant>,
    wait: Duration,
}

impl Default for History {
    fn default() -> Self {
        Self { incomplete: true, again: Some(Instant::now()), wait: FIRST }
    }
}

impl History {
    pub(super) fn incomplete(&self) -> bool {
        self.incomplete
    }

    pub(super) fn again(&self) -> Option<Instant> {
        self.again
    }

    pub(super) fn due(&self) -> bool {
        self.again.is_some_and(|at| Instant::now() >= at)
    }

    pub(super) fn lost(&mut self) {
        self.incomplete = true;
        self.again = Some(Instant::now());
    }

    pub(super) fn retry(&mut self) {
        self.incomplete = true;
        self.again = Some(Instant::now() + self.wait);
        self.wait = (self.wait * 2).min(MOST);
    }

    pub(super) fn settled(&mut self) {
        self.incomplete = false;
        self.again = None;
        self.wait = FIRST;
    }
}
