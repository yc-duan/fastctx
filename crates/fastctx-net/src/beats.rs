//! How regularly a line's heartbeats go and come back: two of them are
//! never more than `HEARTBEAT_LATE` apart under any load
//! (`specs/world-foundation.md`, 地基的数字).

use std::time::Instant;

use fastctx_platform::readings::{Counter, Timing};

use crate::wire::HEARTBEAT_LATE;

/// The gaps between one line's heartbeats of one kind, timed under a name
/// for every line together, with those that came late counted.
pub(crate) struct Beats {
    gaps: &'static Timing,
    late: &'static Counter,
    last: Option<Instant>,
}

impl Beats {
    /// Heartbeats timed as `gaps`, the late ones counted as `late`, from
    /// the second on.
    pub(crate) fn new(gaps: &'static str, late: &'static str) -> Self {
        Self {
            gaps: fastctx_platform::readings::timing(gaps),
            late: fastctx_platform::readings::counter(late),
            last: None,
        }
    }

    /// As `new`, the first timed from now.
    pub(crate) fn from_now(gaps: &'static str, late: &'static str) -> Self {
        Self { last: Some(Instant::now()), ..Self::new(gaps, late) }
    }

    /// One more heartbeat now.
    pub(crate) fn beat(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last.replace(now) {
            let gap = now.duration_since(last);
            self.gaps.record(gap);
            if gap > HEARTBEAT_LATE {
                self.late.add(1);
            }
        }
    }
}
