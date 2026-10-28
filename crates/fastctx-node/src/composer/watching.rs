//! Queue editors are scoped to live subscriptions, without polling leases.

use super::*;

/// Registers before the live subscription is established. Its owner drops it on disconnect.
pub async fn watch(node: &Arc<Node>, id: &ResidentId, name: String) -> Result<Lease, Failure> {
    register(node, id, name, true)
}

pub(super) fn register(node: &Arc<Node>, id: &ResidentId, name: String, legacy: bool) -> Result<Lease, Failure> {
    {
        let mut watches = node.composers.watches.lock().expect("composer watches");
        if name.is_empty() || watches.contains_key(&name) {
            return Err(failure("a fresh nonempty composer watch ID is required"));
        }
        watches.insert(name.clone(), id.clone());
    }
    if legacy {
        *node.composers.legacy_watchers.lock().expect("legacy composer watches").entry(id.clone()).or_default() += 1;
    }
    let lease = Lease { node: node.clone(), id: id.clone(), name, cell: node.composers.cell(id), legacy };
    Ok(lease)
}

pub(super) fn legacy(node: &Node, id: &ResidentId) -> bool {
    node.composers.legacy_watchers.lock().expect("legacy composer watches").contains_key(id)
}

pub async fn snapshot(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    node.watchers.publish(id, &event(cell.as_ref().expect("loaded").view()));
    Ok(())
}

pub struct Lease {
    node: Arc<Node>,
    id: ResidentId,
    name: String,
    cell: Cell,
    legacy: bool,
}

pub(super) fn restore(node: &Node, id: &ResidentId, state: &mut Stored) {
    let editors = node.composers.editors.lock().expect("composer editors");
    for entry in &mut state.queue {
        entry.editors.clear();
        if let Some(watches) = editors.get(id) {
            entry.editors.extend(
                watches.iter().filter(|(_, entries)| entries.contains(&entry.id)).map(|(watch, _)| watch.clone()),
            );
        }
    }
}

pub(super) fn editing(node: &Node, id: &ResidentId, watch: &str, entry: &fastctx_types::NoticeId, editing: bool) {
    let mut editors = node.composers.editors.lock().expect("composer editors");
    if editing {
        editors.entry(id.clone()).or_default().entry(watch.into()).or_default().insert(entry.clone());
    } else if let Some(watches) = editors.get_mut(id) {
        if let Some(entries) = watches.get_mut(watch) {
            entries.remove(entry);
            if entries.is_empty() {
                watches.remove(watch);
            }
        }
        if watches.is_empty() {
            editors.remove(id);
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.node.composers.watches.lock().expect("composer watches").remove(&self.name);
        if self.legacy {
            let mut counts = self.node.composers.legacy_watchers.lock().expect("legacy composer watches");
            if let Some(count) = counts.get_mut(&self.id) {
                *count -= 1;
                if *count == 0 {
                    counts.remove(&self.id);
                }
            }
        }
        let editing = {
            let mut editors = self.node.composers.editors.lock().expect("composer editors");
            match editors.get_mut(&self.id) {
                Some(watches) => {
                    let editing = watches.remove(&self.name).is_some();
                    if watches.is_empty() {
                        editors.remove(&self.id);
                    }
                    editing
                }
                None => false,
            }
        };
        if !editing {
            return;
        }
        let (node, id, cell) = (self.node.clone(), self.id.clone(), self.cell.clone());
        tokio::spawn(async move {
            if !node.residents.knows(&id) {
                return;
            }
            let result = async {
                let mut cell = cell.lock().await;
                load(&node, &id, &mut cell).await?;

                let mut next = cell.take().expect("loaded");
                restore(&node, &id, &mut next);
                publish(&node, &id, &mut cell, next).await
            }
            .await;
            if let Err(error) = result {
                node.log.line(format_args!("closing {id}'s composer editor: {error}"));
            }
            schedule(&node, &id);
        });
    }
}
