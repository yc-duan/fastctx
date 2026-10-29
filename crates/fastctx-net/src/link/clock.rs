//! The hub's clock beside this machine's: the hub says the moment it
//! answers each heartbeat, and the answer that came back soonest of late
//! says best how far apart the two are. The times the hub gives of when a
//! machine came or went are shown by this machine's clock, so how long ago
//! is right however the two clocks differ; nothing is timed, ordered or
//! counted by them.

use std::collections::VecDeque;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};

use super::Inner;
use crate::directory::View;

/// How many of the latest heartbeats' answers it is worked out from.
const SAMPLES: usize = 32;
/// A difference smaller than this is taken for none: clocks kept in time
/// differ by less, and an answer's way there and back blurs it by as much.
const NOTICED: SignedDuration = SignedDuration::from_secs(2);

/// What the hub's latest answers to heartbeats said of its clock.
#[derive(Default)]
pub(super) struct Clock {
    /// Each answer's round trip, and how far ahead the hub's clock was by it.
    samples: VecDeque<(Duration, SignedDuration)>,
}

impl Clock {
    /// How far ahead of this machine's clock the hub's is, as best known;
    /// none when it is not known or too small to tell.
    fn ahead(&self) -> Option<SignedDuration> {
        let (_, ahead) = self.samples.iter().min_by_key(|(trip, _)| *trip)?;
        (ahead.abs() >= NOTICED).then(|| SignedDuration::from_secs(ahead.as_secs()))
    }
}

impl Inner {
    /// The hub answered a heartbeat at `at` by its clock, the answer coming
    /// back `trip` after the heartbeat went.
    pub(super) fn heard_time(&self, at: Timestamp, trip: Duration) {
        let Ok(half) = SignedDuration::try_from(trip / 2) else { return };
        let Ok(here) = Timestamp::now().checked_sub(half) else { return };
        let ahead = at.duration_since(here);
        let mut clock = self.clock.lock().expect("never poisoned");
        if clock.samples.len() == SAMPLES {
            clock.samples.pop_front();
        }
        clock.samples.push_back((trip, ahead));
    }

    /// `at`, a time by the hub's clock, by this machine's.
    pub(super) fn local(&self, at: Timestamp) -> Timestamp {
        let ahead = self.clock.lock().expect("never poisoned").ahead();
        ahead.and_then(|ahead| at.checked_sub(ahead).ok()).unwrap_or(at)
    }

    /// Puts the times in `view` the hub gave by its clock by this machine's.
    pub(super) fn localize(&self, view: &mut View) {
        for member in &mut view.directory.members {
            member.since = member.since.map(|at| self.local(at));
            member.settled = member.settled.map(|at| self.local(at));
        }
    }
}
