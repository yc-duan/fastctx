//! Completion is recorded independently of the foreground reply's lifetime.
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
    if let Some(mut receipt) = receipts::receipt(node, id, state, operation).await? {
        receipt.landed = landed;
        receipt.delivery = Some(flight.delivery.id.clone());
        receipt.failure = failure;
        state.receipts.insert(operation.clone(), receipt);
    } else {
        // A reservation can outlive its ordinary receipt retention. Keep the
        // observed outcome without inventing the original mutation's contents.
        node.log.line(format_args!("{id}'s input {operation}: operation receipt missing; recording its outcome"));
        state.outcomes.insert(
            operation.clone(),
            Operation {
                operation: operation.clone(),
                delivery: Some(flight.delivery.id.clone()),
                accepted: false,
                landed,
                failure,
            },
        );
    }
    Ok(())
}
