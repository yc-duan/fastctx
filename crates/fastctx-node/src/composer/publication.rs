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

pub(super) fn cache(snapshots: &Snapshots, id: &ResidentId, composer: Arc<Composer>) {
    replace(snapshots, id, Cached { revision: composer.revision, view: Some(composer) });
}

pub(super) fn invalidate(node: &Node, id: &ResidentId, revision: u64) {
    replace(&node.composers.snapshots, id, Cached { revision, view: None });
    failures::committed(node, id, revision);
}

fn replace(snapshots: &Snapshots, id: &ResidentId, next: Cached) {
    let mut snapshots = snapshots.lock().expect("composer snapshots");
    if snapshots.get(id).is_some_and(|kept| kept.revision > next.revision) {
        return;
    }
    if snapshots.len() >= 1024
        && !snapshots.contains_key(id)
        && let Some(evicted) = snapshots.keys().next().cloned()
    {
        snapshots.remove(&evicted);
    }
    let previous = snapshots.insert(id.clone(), next);
    drop(snapshots);
    drop(previous);
}

pub(super) fn committed(
    node: &Arc<Node>,
    runtime: &tokio::runtime::Handle,
    id: &ResidentId,
    view: Arc<Composer>,
    busy: bool,
) {
    cache(&node.composers.snapshots, id, view.clone());
    failures::committed(node, id, view.revision);
    {
        let mut pending = node.composers.publications.0.lock().expect("composer publications");
        match pending.entry(id.clone()) {
            Entry::Occupied(mut entry) => {
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
    runtime.spawn(async move {
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
