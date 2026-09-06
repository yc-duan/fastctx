//! Per-user connection and request activity used by the control center's shutdown policy.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct ActivityState {
    accepting_connections: bool,
    connections: usize,
    in_flight: usize,
    last_activity: Instant,
}

/// Shared connection gate, counters, and last-activity timestamp for one control center.
#[derive(Debug)]
pub(crate) struct RuntimeActivity {
    state: Mutex<ActivityState>,
}

impl RuntimeActivity {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ActivityState {
                accepting_connections: true,
                connections: 0,
                in_flight: 0,
                last_activity: Instant::now(),
            }),
        })
    }

    pub(crate) fn try_connection(self: &Arc<Self>) -> Option<ConnectionActivityGuard> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting_connections {
            return None;
        }
        state.connections = state.connections.saturating_add(1);
        state.last_activity = Instant::now();
        Some(ConnectionActivityGuard {
            activity: Arc::clone(self),
        })
    }

    pub(crate) fn request(self: &Arc<Self>) -> RequestActivityGuard {
        let mut state = self.state.lock().unwrap();
        state.in_flight = state.in_flight.saturating_add(1);
        state.last_activity = Instant::now();
        RequestActivityGuard {
            activity: Arc::clone(self),
        }
    }

    pub(crate) fn touch(&self) {
        self.state.lock().unwrap().last_activity = Instant::now();
    }

    /// Current connection/request counts, read without changing shutdown or admission state.
    pub(crate) fn counts(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.connections, state.in_flight)
    }

    pub(crate) fn is_shutdown_eligible(&self, duration: Duration) -> bool {
        self.is_shutdown_eligible_at(Instant::now(), duration)
    }

    pub(crate) fn try_begin_shutdown(&self, duration: Duration) -> bool {
        self.try_begin_shutdown_at(Instant::now(), duration)
    }

    fn is_shutdown_eligible_at(&self, now: Instant, duration: Duration) -> bool {
        let state = self.state.lock().unwrap();
        shutdown_eligible(&state, now, duration)
    }

    fn try_begin_shutdown_at(&self, now: Instant, duration: Duration) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.accepting_connections || !shutdown_eligible(&state, now, duration) {
            return false;
        }
        state.accepting_connections = false;
        true
    }
}

fn shutdown_eligible(state: &ActivityState, now: Instant, duration: Duration) -> bool {
    state.connections == 0
        && state.in_flight == 0
        && now.saturating_duration_since(state.last_activity) >= duration
}

/// One accepted IPC connection, including its handshake. Dropping the lease restarts idle time.
pub(crate) struct ConnectionActivityGuard {
    activity: Arc<RuntimeActivity>,
}

impl Drop for ConnectionActivityGuard {
    fn drop(&mut self) {
        let mut state = self.activity.state.lock().unwrap();
        state.connections = state.connections.saturating_sub(1);
        state.last_activity = Instant::now();
    }
}

/// One in-flight request. Dropping it records completion even on cancellation or panic unwind.
pub(crate) struct RequestActivityGuard {
    activity: Arc<RuntimeActivity>,
}

impl Drop for RequestActivityGuard {
    fn drop(&mut self) {
        let mut state = self.activity.state.lock().unwrap();
        state.in_flight = state.in_flight.saturating_sub(1);
        state.last_activity = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeActivity;
    use std::time::{Duration, Instant};

    #[test]
    fn in_flight_work_prevents_idle_and_completion_restarts_the_clock() {
        let activity = RuntimeActivity::new();
        let guard = activity.request();
        assert!(!activity.is_shutdown_eligible_at(
            Instant::now() + Duration::from_secs(60),
            Duration::from_secs(1)
        ));
        drop(guard);
        assert!(!activity.is_shutdown_eligible_at(Instant::now(), Duration::from_secs(1)));
        assert!(activity.is_shutdown_eligible_at(
            Instant::now() + Duration::from_secs(2),
            Duration::from_secs(1)
        ));
    }

    #[test]
    fn live_connection_prevents_shutdown_and_disconnect_restarts_the_clock() {
        let activity = RuntimeActivity::new();
        let connection = activity.try_connection().unwrap();
        assert!(!activity.is_shutdown_eligible_at(
            Instant::now() + Duration::from_secs(60),
            Duration::from_secs(1)
        ));
        drop(connection);
        assert!(!activity.is_shutdown_eligible_at(Instant::now(), Duration::from_secs(1)));
        assert!(activity.is_shutdown_eligible_at(
            Instant::now() + Duration::from_secs(2),
            Duration::from_secs(1)
        ));
    }

    #[test]
    fn shutdown_decision_closes_the_same_gate_used_by_accept() {
        let activity = RuntimeActivity::new();
        assert!(activity.try_begin_shutdown_at(
            Instant::now() + Duration::from_secs(2),
            Duration::from_secs(1)
        ));
        assert!(activity.try_connection().is_none());
        assert!(!activity.try_begin_shutdown(Duration::ZERO));
    }
}
