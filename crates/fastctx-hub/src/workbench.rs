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

/// `name`'s session `generation` told with a heartbeat the numbers its
/// answers to others stand at (`fastctx_types::numbers`): told beside its
/// presence to whoever may ask it for each kind, as they move, so one keeping
/// what it answered asks again then and not before. None from a release that
/// does not tell them.
pub(crate) fn numbers_heard(
    hub: &Hub,
    name: &MachineName,
    generation: u64,
    told: Option<fastctx_types::numbers::Numbers>,
) {
    let Some(told) = told else { return };
    // A session replaced since does not tell for the one after it.
    if hub.sessions().get(name).is_none_or(|session| session.generation != generation) {
        return;
    }
    let before = hub.logins.lock().expect("never poisoned").insert(name.clone(), (generation, told));
    if before != Some((generation, told)) {
        hub.present_numbers(name);
    }
}

/// `name`'s session `generation` ended: what it told goes with it, so a
/// session of a release that tells none is not taken for one that did.
pub(crate) fn logins_gone(hub: &Hub, name: &MachineName, generation: u64) {
    let mut logins = hub.logins.lock().expect("never poisoned");
    if logins.get(name).is_some_and(|(told_by, _)| *told_by == generation) {
        logins.remove(name);
    }
}

/// The numbers `name` last told its answers stand at.
pub(crate) fn numbers_of(hub: &Hub, name: &MachineName) -> Option<fastctx_types::numbers::Numbers> {
    hub.logins.lock().expect("never poisoned").get(name).map(|(_, told)| *told)
}
