//! When the node lets go of the markers of applied Claude reports
//! (`exchange::compact_applied`): now and then while a session's mod vouches
//! for what it has written, and once its process is gone.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use fastctx_hosts::claude::exchange;

/// Applied reports of one session between two compactions. Compacting reads
/// three directories and syncs one file, so it is not done for every report.
const EVERY: u32 = 64;

#[derive(Default)]
pub(super) struct Markers(Mutex<HashMap<String, Pace>>);

#[derive(Default)]
struct Pace {
    applied: u32,
    /// The latest floor the session's mod declared. An earlier one stays
    /// true, so the latest is the furthest any of them reaches.
    floor: Option<String>,
    /// When the session's `events/` and `accounting/` were last found
    /// holding nothing: the times they were last changed then, and when.
    quiet: Option<(Changed, Instant)>,
}

/// When a session's `events/` and `accounting/` were last changed, each
/// none when it is not there.
pub(super) type Changed = (Option<SystemTime>, Option<SystemTime>);

/// How long ago a directory must have last changed for an unchanged time
/// to say it holds what it held: a file made within the same tick of the
/// file system's clock leaves its time as it was.
const SETTLED: Duration = Duration::from_secs(2);
/// How long a finding that the files held nothing stands without reading
/// them again, whatever their times say: times a file system keeps stale,
/// or reports late, are read past within this.
const QUIET_FOR: Duration = Duration::from_secs(1);

impl Markers {
    /// Notes the reports of `session` applied in one look and the floors
    /// they declared; returns the floor to compact up to when it is time.
    pub(super) fn applied(&self, session: &str, reports: u32, floor: Option<String>) -> Option<String> {
        let mut paces = self.0.lock().expect("never poisoned");
        let pace = paces.entry(session.to_string()).or_default();
        pace.applied = pace.applied.saturating_add(reports);
        if floor.as_ref().is_some_and(|floor| pace.floor.as_ref().is_none_or(|known| floor > known)) {
            pace.floor = floor;
        }
        if pace.applied < EVERY {
            return None;
        }
        let floor = pace.floor.clone()?;
        pace.applied = 0;
        Some(floor)
    }

    /// Whether the session's files are known to hold nothing to take: they
    /// held nothing when last read, and neither directory changed since,
    /// long enough ago to tell.
    pub(super) fn quiet(&self, session: &str, now: Changed) -> bool {
        let settled = [now.0, now.1].into_iter().flatten().all(|at| at.elapsed().is_ok_and(|since| since >= SETTLED));
        settled
            && self
                .0
                .lock()
                .expect("never poisoned")
                .get(session)
                .and_then(|pace| pace.quiet)
                .is_some_and(|(found, at)| found == now && at.elapsed() < QUIET_FOR)
    }

    /// Notes what the session's files held when they were just read:
    /// nothing, as of `now`, or something (`None`).
    pub(super) fn note_quiet(&self, session: &str, now: Option<Changed>) {
        self.0.lock().expect("never poisoned").entry(session.to_string()).or_default().quiet =
            now.map(|now| (now, Instant::now()));
    }

    pub(super) fn forget(&self, session: &str) {
        self.0.lock().expect("never poisoned").remove(session);
    }
}

/// Compacts the markers of a session directory whose registered process has
/// ended. A session without a registration is left as it is: nothing says
/// who may still be writing there.
pub(super) fn compact_ended(session: &Path) -> std::io::Result<usize> {
    let Some(registration) = exchange::registration(session)? else {
        return Ok(0);
    };
    let Some(process) = &registration.process else {
        return Ok(0);
    };
    if fastctx_platform::is_alive(process)? {
        return Ok(0);
    }
    exchange::compact_applied(session, exchange::Vouched::Ended(&registration))
}
