//! Opt-in snapshots of the update records retired by directory changes.
use super::*;

pub(crate) fn seed(hub: &Hub) {
    let store = hub.store();
    let mut updates = hub.updates.lock().expect("updates");
    for member in &store.book.directory.members {
        updates.last.insert(
            member.key,
            Arc::new(history::LastReport {
                at: member.added,
                report: Report { kernel: "1.0.1-beta.17".into(), hub_here: true, ..Default::default() },
            }),
        );
        updates.hub_hosts.insert(member.key);
        updates.history_writer.put(member.key, updates.last.get(&member.key).cloned());
    }
}

pub(crate) fn flush(hub: &Hub) {
    let writer = hub.updates.lock().expect("updates").history_writer.clone();
    tokio::runtime::Handle::current().block_on(writer.flush());
}

pub(crate) fn announce(hub: &Hub) {
    super::announce::tell(hub, true, &Default::default());
}

pub(crate) fn announcements(hub: &Hub) -> serde_json::Value {
    let updates = hub.updates.lock().expect("updates");
    let told: std::collections::BTreeMap<_, _> =
        updates.told.iter().map(|(name, snapshot)| (name, snapshot.materialize())).collect();
    serde_json::json!(told)
}

pub(crate) fn snapshot(hub: &Hub) -> serde_json::Value {
    let updates = hub.updates.lock().expect("updates");
    serde_json::json!({"last": updates.last, "hub_hosts": updates.hub_hosts})
}
