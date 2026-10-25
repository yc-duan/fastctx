//! An official host window opened through the World keeps one connection.

use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct Attached(Arc<Mutex<HashMap<ResidentId, usize>>>);

impl Attached {
    pub(crate) fn contains(&self, id: &ResidentId) -> bool {
        self.0.lock().expect("never poisoned").contains_key(id)
    }
}

pub(crate) struct Lease {
    held: Arc<Mutex<HashMap<ResidentId, usize>>>,
    id: ResidentId,
    changed: Arc<tokio::sync::Notify>,
}

pub(crate) async fn hold(node: &Arc<Node>, id: &ResidentId) -> Result<Lease, Failure> {
    let management = node.residents.management(id)?;
    let _reading = crate::gate_waits::shared(node, id, &management).await?;
    node.residents.active(id)?;
    let held = node.attached.0.clone();
    *held.lock().expect("never poisoned").entry(id.clone()).or_default() += 1;
    node.log.line(format_args!("lifecycle {id}: official interface attached"));
    node.watchers.changed.notify_one();
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
        self.changed.notify_one();
    }
}
