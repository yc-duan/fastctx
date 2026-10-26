//! The engine observes all queued and in-flight input, including entries hidden by the public view.

use super::{Stored, load, storage};
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;

pub(super) fn busy(state: &Stored) -> bool {
    !state.queue.is_empty() || state.sending.is_some() || !state.pending.is_empty() || state.awaiting_turn.is_some()
}

pub(crate) async fn input_busy(node: &Node, id: &ResidentId) -> Result<bool, Failure> {
    let Some(cell) = storage::existing(node, id)? else { return Ok(false) };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell)?;
    Ok(busy(cell.as_ref().expect("loaded composer")))
}
