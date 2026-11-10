//! Scope-local wakeups for standalone histories; network scopes use their link.

use fastctx_types::history::Scope;
use std::{collections::BTreeMap, sync::Mutex};
use tokio::sync::watch;

pub(crate) fn readers(
    node: &crate::Node,
    scope: &Scope,
) -> Vec<(fastctx_types::access::HubId, Vec<fastctx_types::AccountName>)> {
    node.links().iter().map(|link| (link.hub().clone(), link.history_readers(scope))).collect()
}

#[derive(Default)]
pub(crate) struct Changes(Mutex<BTreeMap<String, watch::Sender<u64>>>);
impl Changes {
    pub(crate) fn subscribe(&self, scope: &Scope) -> watch::Receiver<u64> {
        self.0
            .lock()
            .expect("history watches")
            .entry(serde_json::to_string(scope).expect("history scope serializes"))
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }
    pub(crate) fn changed(&self, scope: &Scope) {
        let mut watchers = self.0.lock().expect("history watches");
        watchers.retain(|_, sender| sender.receiver_count() != 0);
        if let Some(sender) = watchers.get(&serde_json::to_string(scope).expect("history scope serializes")) {
            sender.send_modify(|revision| *revision = revision.wrapping_add(1));
        }
    }
}
