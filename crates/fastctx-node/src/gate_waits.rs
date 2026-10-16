//! How long what is asked from outside the node waits for a conversation's
//! management gate (`world-concurrency.md`, 等锁也有期限): the interface, the
//! command line and other machines wait at most `ASKED_WAITS`, then are told
//! what the conversation is doing. What its host reports, and what the node
//! hands over by itself, waits as long as it takes.

use std::time::Duration;

use fastctx_platform::doing::{Busy, RwLock, RwLockReadGuard, RwLockWriteGuard};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;

use crate::Node;

const ASKED_WAITS: Duration = Duration::from_secs(30);

/// A share of `gate`, the management gate of `id`, for something asked.
pub(crate) async fn shared<'a>(
    node: &Node,
    id: &ResidentId,
    gate: &'a RwLock<()>,
) -> Result<RwLockReadGuard<'a, ()>, Failure> {
    gate.read_within(ASKED_WAITS).await.map_err(|busy| told(node, id, busy))
}

/// `gate`, the management gate of `id`, alone, for something asked.
pub(crate) async fn alone<'a>(
    node: &Node,
    id: &ResidentId,
    gate: &'a RwLock<()>,
) -> Result<RwLockWriteGuard<'a, ()>, Failure> {
    gate.write_within(ASKED_WAITS).await.map_err(|busy| told(node, id, busy))
}

fn told(node: &Node, id: &ResidentId, busy: Busy) -> Failure {
    if node.residents.is_deleting(id) {
        return Failure::new(format!("{id} is being deleted"));
    }
    Failure::new(match busy.doing {
        Some(doing) => format!("{id} is busy ({doing}); try again in a moment"),
        None => format!("{id} is busy; try again in a moment"),
    })
}
