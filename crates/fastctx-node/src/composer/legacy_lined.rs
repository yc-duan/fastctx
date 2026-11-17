//! Serves interfaces of 1.0.1-beta.17 and earlier, whose sends are not
//! bounded: they wait for where the words landed, and take an answer without
//! a landing as words that did not go, which they drop from the conversation
//! and, being on their way, hide from the queue too. Words lined up behind
//! others (asked to go at once while others were being handed over, or sent
//! after a stop) are answered to them once they went, as a single send is.
//! Remove once no interface of 1.0.1-beta.17 or earlier is served.

use super::*;

/// The longest such an interface is kept waiting before it is answered with
/// the composer as it stands.
const WAITS: std::time::Duration = std::time::Duration::from_secs(600);

/// Waits until the words `operation` lined up went, failed, or left the line
/// (held for the user, withdrawn): the answer the interface takes.
pub(super) async fn answer(node: &Arc<Node>, id: &ResidentId, operation: &str) -> Result<answer::Answer, Failure> {
    let deadline = tokio::time::Instant::now() + WAITS;
    let mut pause = std::time::Duration::from_millis(25);
    loop {
        {
            let cell = node.composers.cell(id);
            let mut cell = cell.lock().await;
            load(node, id, &mut cell).await?;
            let state = cell.as_ref().expect("loaded");
            let receipt = receipts::receipt(node, id, state, operation).await?;
            if let Some(error) = receipt.as_ref().and_then(|receipt| receipt.failure.clone()) {
                return Err(Failure::new(error));
            }
            let landed = receipt.as_ref().and_then(|receipt| receipt.landed.clone());
            let lined = state.lined.iter().any(|lined| lined.operation.as_deref() == Some(operation));
            let flying = state
                .sending
                .iter()
                .chain(&state.pending)
                .find(|flight| flight.operation.as_deref() == Some(operation))
                .map(|flight| flight.delivery.id.clone());
            let delivery = receipt.as_ref().and_then(|receipt| receipt.delivery.clone()).or(flying.clone());
            if landed.is_some() || (!lined && flying.is_none()) || tokio::time::Instant::now() >= deadline {
                return Ok(answer::Answer::state(state, false, landed, delivery));
            }
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(std::time::Duration::from_millis(500));
    }
}
