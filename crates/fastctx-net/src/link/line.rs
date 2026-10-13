//! How this machine's line to the hub has held, for its details page
//! (`world-gui.md`, 机器的详情): the round trip of each heartbeat over the
//! last hour, kept while the node runs; and when the line came up and went
//! down, and why, kept in `line.json` beside the membership over the last
//! 30 days, at most `KEPT` changes, the oldest going first. The node's own
//! start is kept among them: before it, the machine or its node was not
//! running.

use std::collections::VecDeque;

use fastctx_types::stats::{LineChange, Trip};
use jiff::{SignedDuration, Timestamp};

use super::{Inner, state};

/// The changes kept: a line that drops and comes back every minute for a
/// day fills it, which is more than a page shows.
const KEPT: usize = 2000;

/// How far back changes are kept.
const DAYS: i64 = 30;

/// How many of the line's longest recent round trips a check waits for.
const CHECK_TIMES: u64 = 4;

/// How far back round trips are kept: one each heartbeat.
const TRIPS: SignedDuration = SignedDuration::from_hours(1);

#[derive(Default)]
pub(crate) struct Line {
    trips: VecDeque<Trip>,
    changes: Vec<LineChange>,
}

impl Inner {
    /// Reads the changes kept, and notes that the node started.
    pub(super) fn line_started(&self) {
        let path = self.dir.path().join("line.json");
        let changes = state::read_or(&path, Vec::<LineChange>::new()).unwrap_or_else(|error| {
            self.node.log(format!("reading how the line to the hub held: {error}; it is kept afresh"));
            Vec::new()
        });
        self.line.lock().expect("never poisoned").changes = changes;
        self.line_changed(LineChange::Started { at: Timestamp::now() });
    }

    /// Keeps `change`, and what of the kept changes is recent enough.
    pub(super) fn line_changed(&self, change: LineChange) {
        let mut line = self.line.lock().expect("never poisoned");
        line.changes.push(change);
        let since = Timestamp::now() - SignedDuration::from_hours(24 * DAYS);
        line.changes.retain(|change| change.at() >= since);
        let over = line.changes.len().saturating_sub(KEPT);
        line.changes.drain(..over);
        if let Err(error) = state::write(&self.dir.path().join("line.json"), &line.changes) {
            self.node.log(format!("keeping how the line to the hub held: {error}"));
        }
    }

    /// How long an answer to a check may take before the line is taken for
    /// dead: several times the longest round trip it made over the last
    /// hour, at most its silence (`world-network.md`, 有人在等，就替他看着线).
    /// Before any round trip is measured, it is the silence.
    pub(super) fn check_within(&self) -> std::time::Duration {
        let longest = self.line.lock().expect("never poisoned").trips.iter().map(|trip| trip.ms).max();
        longest.map_or(crate::wire::SILENCE, |ms| {
            std::time::Duration::from_millis(u64::from(ms) * CHECK_TIMES).min(crate::wire::SILENCE)
        })
    }

    /// Keeps one heartbeat's round trip.
    pub(super) fn trip(&self, ms: u32) {
        let at = Timestamp::now();
        let mut line = self.line.lock().expect("never poisoned");
        line.trips.push_back(Trip { at, ms });
        let since = at - TRIPS;
        while line.trips.front().is_some_and(|trip| trip.at < since) {
            line.trips.pop_front();
        }
    }
}

impl super::Link {
    /// The round trips to the hub over the last hour, oldest first.
    pub fn trips(&self) -> Vec<Trip> {
        self.inner.line.lock().expect("never poisoned").trips.iter().copied().collect()
    }

    /// When the line came up and went down over the last 30 days, oldest
    /// first.
    pub fn line_changes(&self) -> Vec<LineChange> {
        self.inner.line.lock().expect("never poisoned").changes.clone()
    }
}
