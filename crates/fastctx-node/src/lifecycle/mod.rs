//! Host lifetime is independent of a conversation's availability and turns.
//! Host-specific evidence belongs in adapters; admission and retention policy
//! live here, shared by all sources of work.

mod adapters;
pub(crate) mod attached;
pub(crate) mod cooldown;
pub(crate) mod refused;
mod retention;
pub(crate) mod retry;
mod sweep;
pub(crate) mod wake;
mod words;

pub(crate) use retention::start;

use crate::Node;
use fastctx_types::{ipc::Failure, residents::About};
use std::sync::Arc;

/// Why the World gives up the host runtime. Deletion also retires the
/// supervisor record; archiving follows the host's own archive contract.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ReleaseReason {
    Idle,
    Sleep,
    Archive,
    Delete,
    StopForDelete,
    Uninstall,
}

/// Releases a runtime outside an existing management operation.
pub(crate) async fn release(node: &Arc<Node>, id: &ResidentId, reason: ReleaseReason) -> Result<bool, Failure> {
    let actions = crate::control_queue::gate(node, id);
    let _action = actions.lock().await;
    let gate = node.residents.management(id)?;
    let _exclusive = gate.write().await;
    let (about, _) = node.residents.about(id).ok_or_else(crate::authorization::no_subject)?;
    release_locked(node, &about, reason, None).await
}

/// The caller owns the conversation's exclusive management gate. The idle
/// collector additionally owns its action gate and supplies fresh evidence.
pub(crate) async fn release_locked(
    node: &Arc<Node>,
    about: &About,
    reason: ReleaseReason,
    candidate: Option<&adapters::Candidate>,
) -> Result<bool, Failure> {
    let adapter = adapters::Adapter::for_conversation(about)
        .ok_or_else(|| Failure::new("this host has no supported lifecycle entrance"))?;
    node.log.line(format_args!("lifecycle {}: releasing host ({reason:?})", about.id));
    let absent = adapter.release(node, about, reason, candidate).await?;
    if absent || matches!(reason, ReleaseReason::Delete) {
        observed(node, &about.id, false, &format!("released: {reason:?}"));
    }
    Ok(absent)
}
use fastctx_types::ResidentId;

/// Records an observed host runtime without changing the conversation's
/// activity or its last turn timestamp. Returns whether an external session left.
pub(crate) fn observed(node: &Node, id: &ResidentId, present: bool, reason: &str) -> bool {
    let changed = node.residents.host_present(id) != present;
    let departed = node.residents.set_host_present(id, present);
    if present {
        retry::changed(node, id);
    }
    if changed {
        node.log.line(format_args!("lifecycle {id}: host {} ({reason})", if present { "present" } else { "absent" }));
    }
    departed
}
