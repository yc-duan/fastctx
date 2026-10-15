//! Queue editors are scoped to live subscriptions, without polling leases.

use super::*;

/// Registers before the live subscription is established. Its owner drops it on disconnect.
pub async fn watch(node: &Arc<Node>, id: &ResidentId, name: String) -> Result<Lease, Failure> {
    {
        let mut watches = node.composers.watches.lock().expect("composer watches");
        if name.is_empty() || watches.contains_key(&name) {
            return Err(failure("a fresh nonempty composer watch ID is required"));
        }
        watches.insert(name.clone(), id.clone());
    }
    let lease = Lease { node: node.clone(), id: id.clone(), name, cell: node.composers.cell(id) };
    Ok(lease)
}

pub async fn snapshot(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell)?;
    node.watchers.publish(id, &event(cell.as_ref().expect("loaded").view()));
    Ok(())
}

pub struct Lease {
    node: Arc<Node>,
    id: ResidentId,
    name: String,
    cell: Cell,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.node.composers.watches.lock().expect("composer watches").remove(&self.name);
        let (node, id, name, cell) = (self.node.clone(), self.id.clone(), self.name.clone(), self.cell.clone());
        tokio::spawn(async move {
            // Deleted since it was watched: nothing of it is kept to edit.
            if !node.residents.knows(&id) {
                return;
            }
            let mut cell = cell.lock().await;
            let Some(state) = cell.as_ref() else { return };
            let mut next = state.clone();
            let mut changed = false;
            for entry in &mut next.queue {
                changed |= entry.editors.remove(&name);
            }
            if changed && let Err(error) = publish(&node, &id, &mut cell, next) {
                if let Some(state) = cell.as_mut() {
                    for entry in &mut state.queue {
                        entry.editors.remove(&name);
                    }
                }
                node.log.line(format_args!("closing {id}'s composer editor: {error}"));
            }
            drop(cell);
            schedule(&node, &id);
        });
    }
}
