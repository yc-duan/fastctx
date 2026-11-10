//! Commit callbacks replace one immutable snapshot and wake at most one publisher
//! per conversation. Serialization and fanout never run on the database writer.

use super::*;
use fastctx_types::composer::Composer;
use std::collections::hash_map::Entry;

pub(super) struct Cached {
    pub revision: u64,
    pub view: Option<Arc<Composer>>,
}
pub(super) type Snapshots = Arc<Mutex<HashMap<ResidentId, Cached>>>;
type Pending = Option<(Arc<Composer>, bool)>;
#[derive(Default)]
pub(super) struct Publications(Mutex<HashMap<ResidentId, Pending>>);

pub(super) fn cache(snapshots: &Snapshots, runtime: &tokio::runtime::Handle, id: &ResidentId, composer: Arc<Composer>) {
    replace(snapshots, runtime, id, Cached { revision: composer.revision, view: Some(composer) });
}

pub(super) fn invalidate(node: &Node, runtime: &tokio::runtime::Handle, id: &ResidentId, revision: u64) {
    replace(&node.composers.snapshots, runtime, id, Cached { revision, view: None });
    failures::committed(node, id, revision);
}

fn replace(snapshots: &Snapshots, runtime: &tokio::runtime::Handle, id: &ResidentId, next: Cached) -> bool {
    let mut snapshots = snapshots.lock().expect("composer snapshots");
    if snapshots.get(id).is_some_and(|kept| kept.revision > next.revision) {
        return false;
    }
    let evicted = if snapshots.len() >= 1024
        && !snapshots.contains_key(id)
        && let Some(evicted) = snapshots.keys().next().cloned()
    {
        snapshots.remove(&evicted)
    } else {
        None
    };
    let previous = snapshots.insert(id.clone(), next);
    drop(snapshots);
    // The last Arc may own a whole legacy view. Releasing it on the writer
    // would make a native keystroke pay for every old character allocation.
    if previous.as_ref().is_some_and(|old| old.view.is_some()) || evicted.as_ref().is_some_and(|old| old.view.is_some())
    {
        fastctx_platform::task::spawn_blocking_on(runtime, move || drop((previous, evicted)));
    }
    true
}

/// Metadata recovery and queue changes must still wake delivery when no legacy
/// observer needs a view. The commit callback only schedules that work.
pub(super) fn settled(node: &Arc<Node>, runtime: &tokio::runtime::Handle, id: &ResidentId, revision: u64, busy: bool) {
    invalidate(node, runtime, id, revision);
    let (node, id) = (Arc::downgrade(node), id.clone());
    fastctx_platform::task::spawn_on(runtime, async move {
        if let Some(node) = node.upgrade()
            && node.residents.active(&id).is_ok()
        {
            if busy {
                crate::goals::engine::input_changed(&node, &id);
            }
            schedule(&node, &id);
        }
    });
}

pub(super) fn committed(
    node: &Arc<Node>,
    runtime: &tokio::runtime::Handle,
    id: &ResidentId,
    view: Arc<Composer>,
    busy: bool,
) {
    if !replace(&node.composers.snapshots, runtime, id, Cached { revision: view.revision, view: Some(view.clone()) }) {
        return;
    }
    failures::committed(node, id, view.revision);
    {
        let mut pending = node.composers.publications.0.lock().expect("composer publications");
        match pending.entry(id.clone()) {
            Entry::Occupied(mut entry) => {
                if entry.get().as_ref().is_some_and(|(previous, _)| previous.revision > view.revision) {
                    return;
                }
                let busy = busy || entry.get().as_ref().is_some_and(|(_, previous)| *previous);
                entry.insert(Some((view, busy)));
                return;
            }
            Entry::Vacant(entry) => {
                entry.insert(Some((view, busy)));
            }
        }
    }
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn_on(runtime, async move {
        loop {
            let next = {
                let mut pending = node.composers.publications.0.lock().expect("composer publications");
                let next = pending.get_mut(&id).and_then(Option::take);
                if next.is_none() {
                    pending.remove(&id);
                }
                next
            };
            let Some((view, busy)) = next else { break };
            if node.residents.knows(&id) {
                if busy {
                    crate::goals::engine::input_changed(&node, &id);
                }
                node.watchers.publish(&id, &event((*view).clone()));
                schedule(&node, &id);
            }
            tokio::task::yield_now().await;
        }
    });
}
