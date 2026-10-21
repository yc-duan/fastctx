//! Withdraws application requests that have not started installing when their
//! durable hub arrangement is cancelled or replaced.

use std::sync::Arc;

use fastctx_net::ids::HubId;
use fastctx_types::update::Stage;
use jiff::Timestamp;

use super::{Asker, application, dir};
use crate::Node;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Origin {
    pub hub: HubId,
    pub at: Timestamp,
}

impl Origin {
    pub fn from_asker(asker: &Asker) -> Option<Self> {
        asker.scheduled_at.map(|at| Self { hub: asker.hub.clone(), at })
    }

    /// None while the arranging hub has not sent this session's versions.
    fn current(&self, node: &Node, version: &str) -> Option<bool> {
        let links = node.links();
        let link = links.iter().find(|link| link.hub() == &self.hub)?;
        let network = link.current_versions()?;
        Some(network.due.get(&link.name()).is_some_and(|due| due.at == self.at && due.version == version))
    }
}

pub(super) fn withdrawn(node: &Node, version: &str, origin: Option<&Origin>) -> bool {
    origin.is_some_and(|origin| origin.current(node, version) == Some(false))
}

pub(super) fn reconcile(node: &Arc<Node>) {
    if application::busy()
        || super::renewing(node).is_some_and(|renewing| renewing.phase != fastctx_types::renewal::Phase::Failed)
    {
        return;
    }
    let mut held = node.updates.held.lock().expect("never poisoned");
    if held.fetching.is_some() || matches!(held.activity, Some(Stage::Installing { .. } | Stage::Renewing { .. })) {
        return;
    }
    let Some(version) = held.take.clone() else { return };
    if !withdrawn(node, &version, held.origin.as_ref()) {
        return;
    }
    if let Err(error) = std::fs::remove_file(dir(node).join("take.json"))
        && error.kind() != std::io::ErrorKind::NotFound
    {
        node.log.line(format_args!("withdrawing the pending application update: {error}"));
        return;
    }
    let _ = std::fs::remove_file(dir(node).join("asked.json"));
    held.take = None;
    held.origin = None;
    if held.chosen.as_ref().is_some_and(|manifest| manifest.version == version) {
        held.chosen = None;
    }
    held.activity = None;
    drop(held);
    super::report(node);
}
