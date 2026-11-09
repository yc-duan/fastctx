//! Attachment references follow the context actually persisted by a native fork.

use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure};
use std::sync::Arc;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(super) struct Inheritance {
    pub _source_gate: crate::leases::Lease<ResidentId, fastctx_platform::doing::RwLock<()>>,
    pub _source: Arc<fastctx_platform::doing::OwnedRwLockReadGuard<()>>,
    pub _target: Arc<crate::data::places::Using>,
}

/// Grants the fork the attachments its confirmed context refers to.
pub(super) async fn inherit(
    node: &Arc<Node>,
    parent: &ResidentId,
    child: &ResidentId,
    carried: super::fork_carried::Carried,
    source: Inheritance,
) -> Result<(), Failure> {
    let (node, parent, child) = (node.clone(), parent.clone(), child.clone());
    fastctx_platform::task::spawn_blocking(move || {
        let _source = source;
        crate::attachments::inherit_texts(&node, &parent, &child, &carried.copied.context_messages)
    })
    .await
    .map_err(failure)?
}
