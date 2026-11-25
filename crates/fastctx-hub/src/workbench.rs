//! Whether one of each machine's logins has the workbench open
//! (`world-layers.md`), as its heartbeats tell: told beside its presence to
//! those who own it, as it changes, and kept while it is away, since nothing
//! but the machine itself changes it.

use fastctx_types::MachineName;

use crate::Hub;

/// `name` told `told` with a heartbeat; none from a release that does not
/// tell it, or a node that does not know yet.
pub(crate) fn heard(hub: &Hub, name: &MachineName, told: Option<bool>) {
    let Some(told) = told else { return };
    let before = hub.workbench.lock().expect("never poisoned").insert(name.clone(), told);
    if before != Some(told) {
        hub.present(name);
    }
}

/// What `name` last told, if it ever did since the hub started.
pub(crate) fn of(hub: &Hub, name: &MachineName) -> Option<bool> {
    hub.workbench.lock().expect("never poisoned").get(name).copied()
}

/// `name` told with a heartbeat the number its agents and logins stand at
/// (`fastctx_net::wire::Up::Ping::logins_number`): told beside its presence to
/// whoever sees it online, as it moves, so one keeping what it answered of
/// them asks again then and not before. None from a release that does not
/// tell it.
pub(crate) fn logins_heard(hub: &Hub, name: &MachineName, told: Option<u64>) {
    let Some(told) = told else { return };
    let before = hub.logins.lock().expect("never poisoned").insert(name.clone(), told);
    if before != Some(told) {
        hub.present(name);
    }
}

/// The number `name` last told its agents and logins stand at.
pub(crate) fn logins_of(hub: &Hub, name: &MachineName) -> Option<u64> {
    hub.logins.lock().expect("never poisoned").get(name).copied()
}
