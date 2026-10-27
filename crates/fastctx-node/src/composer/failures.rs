//! A failed checkpoint must not release queued words using the previous state.
//! This stop is operational state, independent of tentative document contents.

use super::*;

pub(super) type Holds = Mutex<HashMap<ResidentId, (u64, String)>>;

pub(super) fn hold(node: &Node, id: &ResidentId, revision: u64, reason: String) {
    let mut holds = node.composers.holds.lock().expect("composer checkpoint failures");
    // A timeout can race a successful commit. Its callback updates the cache
    // before taking this lock, so a known commit never becomes an unknown one.
    if node
        .composers
        .snapshots
        .lock()
        .expect("composer snapshots")
        .get(id)
        .is_some_and(|snapshot| snapshot.revision >= revision)
    {
        return;
    }
    if holds.get(id).is_none_or(|(previous, _)| *previous <= revision) {
        holds.insert(id.clone(), (revision, reason));
    }
}

pub(super) fn apply(node: &Node, id: &ResidentId, state: &mut Stored) {
    if let Some((_, reason)) = node.composers.holds.lock().expect("composer checkpoint failures").get(id) {
        state.held = Some(reason.clone());
    }
}

pub(super) fn committed(node: &Node, id: &ResidentId, revision: u64) {
    let mut holds = node.composers.holds.lock().expect("composer checkpoint failures");
    if holds.get(id).is_some_and(|(failed, _)| *failed <= revision) {
        holds.remove(id);
    }
}
