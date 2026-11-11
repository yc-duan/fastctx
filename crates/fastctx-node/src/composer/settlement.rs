//! Completion is recorded independently of the foreground reply's lifetime.
//! What a device reads of its send is one of three things: the words went
//! out (`landed`, with the id they went under), they did not (`failure`),
//! or they are in the queue still, to go later (neither, and no id).
use super::*;
use fastctx_types::composer::sync::Operation;

pub(super) async fn record(
    node: &Node,
    id: &ResidentId,
    state: &mut Stored,
    flight: &Flight,
    result: &Result<Option<Landed>, Failure>,
) -> Result<(), Failure> {
    let Some(operation) = &flight.operation else { return Ok(()) };
    let landed = result.as_ref().ok().cloned().flatten();
    let failure = result.as_ref().err().map(|error| error.message.clone());
    // The id names words that went out. Words still queued, or not sent,
    // went under none: a device must not read one as their having gone.
    let delivery = landed.as_ref().map(|_| flight.delivery.id.clone());
    if let Some(mut receipt) = receipts::receipt(node, id, state, operation).await? {
        receipt.landed = landed;
        receipt.delivery = delivery;
        receipt.failure = failure;
        state.receipts.insert(operation.clone(), receipt);
    } else {
        // A reservation can outlive its ordinary receipt retention. Keep the
        // observed outcome without inventing the original mutation's contents.
        node.log.line(format_args!("{id}'s input {operation}: operation receipt missing; recording its outcome"));
        state.outcomes.insert(
            operation.clone(),
            Operation { operation: operation.clone(), delivery, accepted: false, landed, failure },
        );
    }
    Ok(())
}
