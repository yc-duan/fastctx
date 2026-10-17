//! How a source of a package is judged while it brings it, the same for the
//! hub, a machine fetching for the network and one bringing its own update
//! (`world-network.md`, 中枢替网络存的东西): a source that cannot be reached
//! five times running is no use; one that, at the speed it has gone since
//! its first byte, would need more than five minutes for the rest is slow,
//! and the next is tried beside it; one that brings nothing for half a
//! minute has stopped.

use std::time::{Duration, Instant};

/// Connections that fail running before a source is given up.
pub const TRIES: u32 = 5;
/// How long a source goes from its first byte before its speed is judged.
pub const SAMPLE: Duration = Duration::from_secs(30);
/// Within how long a source must look like finishing not to be slow.
pub const SOON: Duration = Duration::from_secs(5 * 60);
/// How long a source may bring nothing once bytes came, before it stopped.
pub const QUIET: Duration = Duration::from_secs(30);
/// How many sources bring one package at once.
pub const AT_ONCE: usize = 3;

/// What a source looks like now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It goes, or has not had its time yet.
    Promising,
    /// At its speed it would take too long: another is tried beside it.
    Slow,
    /// It brought nothing more for `QUIET`.
    Stopped,
}

/// The bytes one source brought, as they came.
#[derive(Clone, Copy, Debug)]
pub struct Pace {
    started: Instant,
    first: Option<(Instant, u64)>,
    last: (Instant, u64),
}

impl Pace {
    pub fn new(now: Instant) -> Self {
        Self { started: now, first: None, last: (now, 0) }
    }

    /// The source has brought `brought` bytes by `now`.
    pub fn heard(&mut self, now: Instant, brought: u64) {
        match self.first {
            None => {
                if brought > 0 {
                    self.first = Some((now, brought));
                    self.last = (now, brought);
                }
            }
            Some(_) => {
                if brought > self.last.1 {
                    self.last = (now, brought);
                }
            }
        }
    }

    /// How it looks at `now` for a package of `total` bytes, when known: a
    /// source with nothing yet has `before_first` to start.
    pub fn verdict(&self, now: Instant, total: Option<u64>, before_first: Duration) -> Verdict {
        let Some((first_at, first)) = self.first else {
            return if now.duration_since(self.started) >= before_first { Verdict::Slow } else { Verdict::Promising };
        };
        let (last_at, brought) = self.last;
        if total.is_some_and(|total| brought >= total) {
            return Verdict::Promising;
        }
        if now.duration_since(last_at) >= QUIET {
            return Verdict::Stopped;
        }
        let going = now.duration_since(first_at);
        if going < SAMPLE {
            return Verdict::Promising;
        }
        let Some(total) = total else { return Verdict::Promising };
        let speed = (brought - first) as f64 / going.as_secs_f64();
        let rest = total.saturating_sub(brought) as f64;
        if speed <= 0.0 || rest / speed > SOON.as_secs_f64() { Verdict::Slow } else { Verdict::Promising }
    }

    /// Its speed in bytes a second since its first byte; none until more
    /// came after that.
    pub fn speed(&self) -> Option<u64> {
        let (first_at, first) = self.first?;
        let (last_at, brought) = self.last;
        let going = last_at.duration_since(first_at).as_secs_f64();
        (brought > first && going > 0.0).then(|| ((brought - first) as f64 / going) as u64)
    }
}
