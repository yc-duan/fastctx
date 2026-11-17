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
    settlement::record(node, id, &mut next, &flight, &Err(Failure::new(why))).await?;
    publish(node, id, &mut cell, next).await
}
