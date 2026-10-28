//! Coalesced projection refreshes. Each conversation waits only at its own
//! management gate; account invalidations carry no context bodies.

use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct Updates(Mutex<BTreeMap<ResidentId, Pending>>);

struct Pending {
    revision: u64,
    loaded: u64,
    working: bool,
}
impl Default for Pending {
    fn default() -> Self {
        Self { revision: 1, loaded: 0, working: false }
    }
}

/// Called while the conversation's exclusive management gate is held.
pub(crate) async fn refresh(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let revision = {
        let mut pending = node.contexts.0.lock().expect("never poisoned");
        let pending = pending.entry(id.clone()).or_default();
        if pending.loaded == pending.revision {
            return Ok(());
        }
        pending.revision
    };
    node.residents.active(id)?;
    let (about, town) = node.residents.about(id).ok_or_else(|| Failure::new(super::words::UNKNOWN))?;
    crate::focus::refresh(node, id).await?;
    super::refresh_local(node, id).map_err(|error| Failure::new(error.to_string()))?;
    crate::plan::context(node, id, &about.origin, &about.acting).await?;
    super::map(node, id, town.as_ref(), &about.origin, &about.acting).await?;
    node.contexts.0.lock().expect("never poisoned").entry(id.clone()).or_default().loaded = revision;
    Ok(())
}

pub(crate) fn request(node: &Arc<Node>, id: &ResidentId) {
    {
        let mut pending = node.contexts.0.lock().expect("never poisoned");
        let pending = pending.entry(id.clone()).or_default();
        pending.revision = pending.revision.wrapping_add(1);
        if pending.working {
            return;
        }
        pending.working = true;
    }
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
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
            let mut pending = node.contexts.0.lock().expect("never poisoned");
            if node.residents.active(&id).is_err() {
                pending.remove(&id);
                break;
            }
            let pending = pending.get_mut(&id).expect("worker owns its entry");
            if failed || pending.loaded == pending.revision {
                pending.working = false;
                break;
            }
        }
    });
}

pub(crate) fn request_all(node: &Arc<Node>) {
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        request(node, &resident.id);
    }
}

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut views = node.views.subscribe();
        let mut ways = node.ways.subscribe();
        let mut memory = node.memory_changes.subscribe();
        loop {
            request_all(&node);
            tokio::select! {
                _ = views.changed() => {},
                _ = ways.changed() => {},
                _ = memory.changed() => {},
                _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {},
            }
        }
    });
}
