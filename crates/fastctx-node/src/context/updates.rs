//! Coalesced projection refreshes. Each conversation waits only at its own
//! management gate; account invalidations carry no context bodies.

use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// How long a refresh of every conversation waits for the changes that come
/// with the one that woke it: machines come and go in bursts (a room starting
/// up, a line that keeps dropping), and each would otherwise refresh every
/// conversation here again.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct Updates(Mutex<BTreeMap<ResidentId, Pending>>);

struct Pending {
    revision: u64,
    loaded: u64,
    plan_revision: u64,
    plan_loaded: u64,
    working: bool,
}
impl Default for Pending {
    fn default() -> Self {
        Self { revision: 1, loaded: 0, plan_revision: 1, plan_loaded: 0, working: false }
    }
}

/// Called while the conversation's exclusive management gate is held.
pub(crate) async fn refresh(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let (revision, plan_revision, read_plan) = {
        let mut pending = node.contexts.0.lock().expect("never poisoned");
        let pending = pending.entry(id.clone()).or_default();
        if pending.loaded == pending.revision {
            return Ok(());
        }
        (pending.revision, pending.plan_revision, pending.plan_loaded != pending.plan_revision)
    };
    node.residents.active(id)?;
    let (about, town) = node.residents.about(id).ok_or_else(|| Failure::new(super::words::UNKNOWN))?;
    crate::focus::refresh(node, id).await?;
    super::refresh_local(node, id).map_err(|error| Failure::new(error.to_string()))?;
    super::sources::plan_guide(node, id, &about.origin, &about.acting)
        .await
        .map_err(|error| Failure::new(error.to_string()))?;
    if read_plan {
        crate::plan::context(node, id, &about.origin, &about.acting).await?;
    }
    super::sources::skills(node, id, &about.acting).await?;
    super::map(node, id, town.as_ref(), &about.origin, &about.acting).await?;
    let mut contexts = node.contexts.0.lock().expect("never poisoned");
    let pending = contexts.entry(id.clone()).or_default();
    pending.loaded = revision;
    pending.plan_loaded = plan_revision;
    Ok(())
}

fn request(node: &Arc<Node>, id: &ResidentId, plan: bool) {
    {
        let mut pending = node.contexts.0.lock().expect("never poisoned");
        let pending = pending.entry(id.clone()).or_default();
        pending.revision = pending.revision.wrapping_add(1);
        if plan {
            pending.plan_revision = pending.plan_revision.wrapping_add(1);
        }
        if pending.working {
            return;
        }
        pending.working = true;
    }
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
        // Named in what the node reports of a lock held too long.
        let _doing = fastctx_platform::doing::doing(format!("refreshing {id}'s context"));
        let mut again = crate::again::Again::default();
        loop {
            let result = async {
                let gate = node.residents.management(&id)?;
                let _one = gate.write().await;
                refresh(&node, &id).await
            }
            .await;
            let failed = result.is_err();
            if let Err(error) = result
                && node.residents.active(&id).is_ok()
            {
                node.log.line(format_args!("refreshing {id}'s context: {error}"));
            }
            {
                let mut pending = node.contexts.0.lock().expect("never poisoned");
                if node.residents.active(&id).is_err() {
                    pending.remove(&id);
                    break;
                }
                let pending = pending.get_mut(&id).expect("worker owns its entry");
                if !failed && pending.loaded == pending.revision {
                    pending.working = false;
                    break;
                }
            }
            // A refresh that failed is tried again, later each time, and a
            // change meanwhile is taken in with it; one that a change
            // outdated while it ran runs again at once.
            if failed {
                tokio::time::sleep(again.next()).await;
            } else {
                again.reset();
            }
        }
    });
}

/// Refreshes `id`'s context with its plan read again: its plan or working
/// place changed.
pub(crate) fn request_plan(node: &Arc<Node>, id: &ResidentId) {
    request(node, id, true);
}

fn request_each(node: &Arc<Node>, plan: bool) {
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        request(node, &resident.id, plan);
    }
}

pub(crate) fn start(node: &Arc<Node>) {
    super::records::start(node);
    let node = node.clone();
    tokio::spawn(async move {
        let mut views = node.views.subscribe();
        let mut ways = node.ways.subscribe();
        let mut memory = node.memory_changes.subscribe();
        let mut plan = true;
        loop {
            request_each(&node, plan);
            // Connectivity changes the authorized focus and memory paths, but
            // never the plan. Plan records and reconnects invalidate it
            // separately; a refresh that fails is tried again on its own.
            plan = tokio::select! {
                _ = views.changed() => true,
                _ = ways.changed() => false,
                _ = memory.changed() => false,
            };
            tokio::time::sleep(SETTLE).await;
            // What changed meanwhile is in this refresh, and is not seen again.
            plan |= views.has_changed().unwrap_or(false);
            views.mark_unchanged();
            ways.mark_unchanged();
            memory.mark_unchanged();
        }
    });
}
