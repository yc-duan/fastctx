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
