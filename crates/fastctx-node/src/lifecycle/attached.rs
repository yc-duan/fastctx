//! An official host window opened through the World keeps one connection.

use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// The windows open now, by conversation. They keep nothing of its host:
/// that is kept for the model's activity alone (`retention`).
#[derive(Default)]
pub(crate) struct Attached(Arc<Mutex<HashMap<ResidentId, usize>>>);

pub(crate) struct Lease {
    held: Arc<Mutex<HashMap<ResidentId, usize>>>,
    id: ResidentId,
    changed: Arc<crate::watch::Changed>,
}

pub(crate) async fn hold(node: &Arc<Node>, id: &ResidentId) -> Result<Lease, Failure> {
    let management = node.residents.management(id)?;
    let _reading = crate::gate_waits::shared(node, id, &management).await?;
    node.residents.active(id)?;
    let held = node.attached.0.clone();
    *held.lock().expect("never poisoned").entry(id.clone()).or_default() += 1;
    node.log.line(format_args!("lifecycle {id}: official interface attached"));
    node.watchers.changed.tell(id);
    Ok(Lease { held, id: id.clone(), changed: node.watchers.changed.clone() })
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut held = self.held.lock().expect("never poisoned");
        if let Some(count) = held.get_mut(&self.id) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.id);
            }
        }
        self.changed.tell(&self.id);
    }
}
