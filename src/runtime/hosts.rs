//! The host processes a control center keeps its shared runtime warm for.
//!
//! A control center is a cache in front of the host applications using it, so its life belongs to
//! them rather than to a clock. Every proxy names the host process it serves during the handshake;
//! the control center remembers those identities and refuses to shut down while any of them is
//! still running, because that host can open a new conversation — and therefore a new proxy — at
//! any moment, and a host whose stdio MCP server has died never gets it back.

use crate::process_identity::{ProcessIdentity, identity_is_alive};
use std::sync::Mutex;

/// Upper bound on remembered hosts. Dead entries are pruned on every liveness sweep, so this only
/// caps a pathological burst of short-lived hosts between two sweeps.
const MAX_TRACKED_HOSTS: usize = 256;

/// Host identities seen by one control center, with the exited ones pruned as they are noticed.
#[derive(Debug)]
pub(crate) struct HostRegistry {
    hosts: Mutex<Vec<ProcessIdentity>>,
}

impl HostRegistry {
    pub(crate) fn new() -> Self {
        Self {
            hosts: Mutex::new(Vec::new()),
        }
    }

    /// Records the host a newly accepted session belongs to.
    ///
    /// A proxy that could not name its host — the documented parent-watch opt-out, or a platform
    /// where the identity is unavailable — passes `None`. Such a session pins nothing: the control
    /// center could never observe that host exiting, so counting it would mean never exiting.
    pub(crate) fn remember(&self, host: Option<ProcessIdentity>) {
        let Some(host) = host else {
            return;
        };
        let mut hosts = self.hosts.lock().unwrap();
        if hosts.contains(&host) {
            return;
        }
        if hosts.len() >= MAX_TRACKED_HOSTS {
            hosts.retain(identity_is_alive);
        }
        if hosts.len() < MAX_TRACKED_HOSTS {
            hosts.push(host);
        }
    }

    /// Prunes hosts that have exited and reports whether any remembered host is still running.
    ///
    /// Inspects live processes, so callers run it off the async runtime.
    pub(crate) fn prune_and_check(&self) -> bool {
        let snapshot: Vec<ProcessIdentity> = self.hosts.lock().unwrap().clone();
        let alive: Vec<ProcessIdentity> = snapshot.into_iter().filter(identity_is_alive).collect();
        let mut hosts = self.hosts.lock().unwrap();
        // Keep entries added while the snapshot was being probed; only drop the proven-dead ones.
        hosts.retain(|host| alive.contains(host));
        !hosts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::HostRegistry;

    #[test]
    fn a_session_without_a_host_identity_pins_nothing() {
        let registry = HostRegistry::new();

        registry.remember(None);

        assert!(!registry.prune_and_check());
    }

    #[test]
    fn a_live_host_is_remembered_once_and_keeps_the_control_center_open() {
        let registry = HostRegistry::new();
        let running = crate::process_identity::process_identity(std::process::id())
            .expect("the running test process has an inspectable identity");

        registry.remember(Some(running.clone()));
        registry.remember(Some(running));

        assert!(registry.prune_and_check());
    }

    #[test]
    fn an_exited_host_is_pruned_and_stops_pinning() {
        let registry = HostRegistry::new();
        let mut recycled = crate::process_identity::process_identity(std::process::id())
            .expect("the running test process has an inspectable identity");
        recycled.started.push('x');

        registry.remember(Some(recycled));

        assert!(!registry.prune_and_check());
    }
}
