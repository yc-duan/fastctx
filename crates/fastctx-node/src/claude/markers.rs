//! When the node lets go of the markers of applied Claude reports
//! (`exchange::compact_applied`): now and then while a session's mod vouches
//! for what it has written, and once its process is gone.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

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
}

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
