//! Atomic composer checkpoints in the conversation's directory.

use super::*;

/// Lifecycle hooks leave conversations without a shared composer untouched.
pub(super) fn existing(node: &Node, id: &ResidentId) -> Result<Option<Cell>, Failure> {
    if let Some(cell) = node.composers.cells.get(id) {
        return Ok(Some(cell));
    }
    if !node.residents.knows(id) {
        return Ok(None);
    }
    let directory = node.world.conversation(id);
    let _transition = crate::resident_storage::lock();
    if directory.composer().try_exists().map_err(failure)? { Ok(Some(node.composers.cell(id))) } else { Ok(None) }
}

pub(super) fn load(node: &Node, id: &ResidentId, cell: &mut Option<Stored>) -> Result<(), Failure> {
    if cell.is_some() {
        return Ok(());
    }
    if !node.residents.knows(id) {
        return Err(failure("unknown resident"));
    }
    let directory = node.world.conversation(id);
    let _transition = crate::resident_storage::lock();
    let state: Stored = match std::fs::read(directory.composer()) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(failure)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Stored::default(),
        Err(error) => return Err(failure(error)),
    };
    let mut compacted = state.clone();
    receipts::compact(&directory, &state, &mut compacted)?;
    *cell = Some(compacted);
    Ok(())
}

pub(super) fn publish(
    node: &Node,
    id: &ResidentId,
    cell: &mut Option<Stored>,
    mut next: Stored,
) -> Result<(), Failure> {
    next.revision = next.revision.checked_add(1).ok_or_else(|| failure("revision overflow"))?;
    if !node.residents.knows(id) {
        return Err(failure("unknown resident"));
    }
    let directory = node.world.conversation(id);
    let _transition = crate::resident_storage::lock();
    if let Some(previous) = cell.as_ref() {
        receipts::compact(&directory, previous, &mut next)?;
    }
    let bytes = serde_json::to_vec(&next).map_err(failure)?;
    if let Err(error) = fastctx_platform::atomic::publish_checked(
        &directory.composer(),
        &bytes,
        fastctx_platform::Permissions::Private,
        true,
        || Ok(()),
    ) {
        if error.published {
            *cell = Some(next);
        }
        let error = failure(error.error);
        if let Some(state) = cell.as_mut() {
            state.held = Some(error.message.clone());
        }
        drop(_transition);
        // An uncommitted stop cannot be reconstructed from the checkpoint yet.
        if node.residents.removed(id).is_none()
            && let Some(held) = node.composers.cells.get(id)
        {
            node.composers.failed.lock().expect("failed composer checkpoints").insert(id.clone(), held);
        }
        node.watchers.publish(
            id,
            &SessionEvent {
                at: jiff::Timestamp::now(),
                happened: Happened::Changed {
                    turn: None,
                    change: SessionChange::ComposerFailed { error: error.message.clone() },
                },
            },
        );
        return Err(error);
    }
    let view = next.view();
    *cell = Some(next);
    node.composers.failed.lock().expect("failed composer checkpoints").remove(id);
    node.watchers.publish(id, &event(view));
    Ok(())
}

pub(super) fn event(composer: fastctx_types::composer::Composer) -> SessionEvent {
    SessionEvent {
        at: jiff::Timestamp::now(),
        happened: Happened::Changed { turn: None, change: SessionChange::Composer { composer: Box::new(composer) } },
    }
}

/// Restores delivery reservations before automatic release starts.
pub async fn recover(node: &Arc<Node>) -> Result<(), Failure> {
    for resident in node.residents.everyone() {
        if resident.removed.is_some() {
            continue;
        }
        let directory = node.world.conversation(&resident.id);
        if !directory.composer().try_exists().map_err(failure)? {
            continue;
        }
        let cell = node.composers.cell(&resident.id);
        let mut cell = cell.lock().await;
        load(node, &resident.id, &mut cell)?;
        let mut next = cell.as_ref().expect("loaded").clone();
        let flights: Vec<_> = next.sending.take().into_iter().chain(next.pending.clone()).collect();
        for flight in flights {
            if let Some(turn) = crate::delivery::input_turn(&directory, &flight.delivery.id).map_err(failure)? {
                next.queue.retain(|entry| entry.id != flight.entry);
                next.pending.retain(|pending| pending.delivery.id != flight.delivery.id);
                if flight.automatic && !receipts::ended(node, &resident.id, &next, &turn)? {
                    next.awaiting_turn = Some(turn);
                }
            } else {
                let why = crate::delivery::input_uncertainty(&directory, &flight.delivery.id).map_err(failure)?;
                if let Some(why) = why {
                    next.pending.retain(|pending| pending.delivery.id != flight.delivery.id);
                    next.held = Some(why.clone());
                    if let Some(entry) = next.queue.iter_mut().find(|entry| entry.id == flight.entry) {
                        entry.submitted = Some(flight.delivery.id);
                        entry.error = Some(why);
                    }
                    next.retire_unobserved(&flight.entry);
                } else if !next.pending.iter().any(|pending| pending.delivery.id == flight.delivery.id) {
                    // No consuming turn was acknowledged before the node stopped.
                    // Return a Waiting inbox entry to the composer instead of batching it later.
                    crate::inbox::withdraw(&directory, &flight.delivery.id).map_err(failure)?;
                    next.held = Some(
                        "the node restarted before a host submission was recorded; this message remains queued".into(),
                    );
                    if let Some(entry) = next.queue.iter_mut().find(|entry| entry.id == flight.entry) {
                        entry.error = next.held.clone();
                    }
                }
            }
        }
        publish(node, &resident.id, &mut cell, next)?;
        drop(cell);
        uploads::recover(node, &resident.id).await?;
        schedule(node, &resident.id);
    }
    Ok(())
}
