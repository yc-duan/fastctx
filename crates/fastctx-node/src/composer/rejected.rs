//! Return a previously accepted reservation when its host can never consume it.

use super::*;

pub(crate) async fn rejected(
    node: &Arc<Node>,
    id: &ResidentId,
    delivery: &fastctx_types::NoticeId,
    why: &str,
) -> Result<(), Failure> {
    let Some(cell) = storage::existing(node, id).await? else {
        return Ok(());
    };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    let Some(flight) = next.pending.iter().find(|flight| &flight.delivery.id == delivery).cloned() else {
        // An in-flight dispatch returns the refusal to its original caller itself.
        return Ok(());
    };
    next.pending.retain(|flight| &flight.delivery.id != delivery);
    next.held = Some(why.into());
    if let Some(entry) = next.queue.iter_mut().find(|entry| entry.id == flight.entry) {
        entry.submitted = None;
        entry.error = Some(why.into());
    }
    if let Some(operation) = &flight.operation
        && let Some(receipt) = receipts::receipt(node, id, &next, operation).await?
    {
        next.receipts.insert(operation.clone(), receipt);
    }
    for receipt in next.receipts.values_mut() {
        if receipt.delivery.as_ref() == Some(delivery) {
            receipt.failure = Some(why.into());
            receipt.landed = None;
        }
    }
    publish(node, id, &mut cell, next).await
}
