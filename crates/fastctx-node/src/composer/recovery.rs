//! Startup needs delivery reservations, not every idle draft's text model.
//! Cold drafts migrate lazily when opened, or one at a time in the background.

use super::*;
use fastctx_storage::rusqlite::OptionalExtension;
use serde::{Deserialize, de::IgnoredAny};

#[derive(Deserialize)]
struct Pending {
    queue: Vec<IgnoredAny>,
    sending: Option<IgnoredAny>,
    pending: Vec<IgnoredAny>,
    uploads: HashMap<String, IgnoredAny>,
    awaiting_turn: Option<String>,
}

pub(super) async fn needed(node: &Node, id: &ResidentId) -> Result<bool, Failure> {
    let owner = id.to_string();
    let file = node.world.conversation(id).composer();
    node.database
        .read(move |connection| {
            let stored: Option<Vec<u8>> = connection
                .query_row("SELECT content FROM composers WHERE owner=?1", [&owner], |row| row.get(0))
                .optional()?;
            let bytes = match stored {
                Some(bytes) => bytes,
                None => match std::fs::read(file) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                    Err(error) => return Err(error.into()),
                },
            };
            let pending: Pending =
                serde_json::from_slice(&bytes).map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
            Ok(!pending.queue.is_empty()
                || pending.sending.is_some()
                || !pending.pending.is_empty()
                || !pending.uploads.is_empty()
                || pending.awaiting_turn.is_some())
        })
        .await
        .map_err(failure)
}

/// Local reconciliation is enabled only after startup has restored host facts.
pub(super) fn pending(node: &Node, id: &ResidentId) -> bool {
    node.startup.is_ready(id) && node.composers.recovering.lock().expect("composer recovery").contains(id)
}

pub(super) fn finished(node: &Node, id: &ResidentId) {
    node.composers.recovering.lock().expect("composer recovery").remove(id);
}

pub(super) async fn settle(node: &Node, id: &ResidentId, state: &mut Stored) -> Result<bool, Failure> {
    let directory = node.world.conversation(id);
    let changed = state.sending.is_some() || !state.pending.is_empty();
    let flights: Vec<_> = state.sending.take().into_iter().chain(state.pending.clone()).collect();
    for flight in flights {
        if let Some(turn) = crate::delivery::input_turn(&directory, &flight.delivery.id).map_err(failure)? {
            state.queue.retain(|entry| entry.id != flight.entry);
            state.pending.retain(|pending| pending.delivery.id != flight.delivery.id);
            if flight.automatic && !receipts::ended(node, id, state, &turn).await? {
                state.awaiting_turn = Some(turn);
            }
        } else {
            let why = crate::delivery::input_uncertainty(&directory, &flight.delivery.id).map_err(failure)?;
            if let Some(why) = why {
                state.pending.retain(|pending| pending.delivery.id != flight.delivery.id);
                state.held = Some(why.clone());
                if let Some(entry) = state.queue.iter_mut().find(|entry| entry.id == flight.entry) {
                    entry.submitted = Some(flight.delivery.id);
                    entry.error = Some(why);
                }
                state.retire_unobserved(&flight.entry);
            } else if !state.pending.iter().any(|pending| pending.delivery.id == flight.delivery.id) {
                let kept = crate::inbox::read(&directory, &flight.delivery.id).map_err(failure)?;
                if kept.as_ref().is_some_and(|kept| kept.state != fastctx_types::residents::PendingState::Waiting) {
                    // On its way to the host when the node stopped: whether
                    // it went in is for the delivery's own recovery to say
                    // (`speech::recover`), which tells the input as it does.
                    state.pending.push(flight);
                    continue;
                }
                // No consuming turn was acknowledged before the node stopped.
                // Return a Waiting inbox entry to the composer instead of batching it later.
                crate::inbox::withdraw(&directory, &flight.delivery.id).map_err(failure)?;
                state.held = Some(
                    "the node restarted before a host submission was recorded; this message remains queued".into(),
                );
                if let Some(entry) = state.queue.iter_mut().find(|entry| entry.id == flight.entry) {
                    entry.error = state.held.clone();
                }
            }
        }
    }
    Ok(changed)
}

pub(super) fn migrate(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut previous = String::new();
        loop {
            let mut failed = false;
            let mut moved = 0;
            for resident in node.residents.everyone() {
                if resident.removed.is_some() {
                    continue;
                }
                let result = async {
                    if pending(&node, &resident.id) && needed(&node, &resident.id).await? {
                        storage::recover_one(&node, &resident.id).await?;
                        return Ok(true);
                    }
                    migrate_one(&node, &resident.id).await
                }
                .await;
                match result {
                    Ok(true) => moved += 1,
                    Ok(false) => {}
                    Err(error) => {
                        failed = true;
                        if previous != error.message {
                            node.log.line(format_args!("recovering or migrating a composer: {error}; will retry"));
                            previous = error.message;
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            if moved > 0 {
                node.log.line(format_args!("recovered or migrated {moved} composers"));
            }
            if !failed {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

async fn migrate_one(node: &Arc<Node>, id: &ResidentId) -> Result<bool, Failure> {
    let gate = node.residents.management(id)?;
    let _reading = gate.read().await;
    if node.residents.active(id).is_err() {
        return Ok(false);
    }
    let Some(cell) = storage::existing(node, id).await? else {
        finished(node, id);
        return Ok(false);
    };
    let cell = cell.lock().await;
    if cell.is_some() {
        finished(node, id);
        return Ok(false);
    }
    if persistence::exists(node, id).await? {
        storage::forget_file(node, id);
        finished(node, id);
        return Ok(false);
    }
    let mut state = persistence::read(node, id).await?;
    if state.persisted {
        return Ok(false);
    }
    let prepared = persistence::prepare(id, &mut state)?;
    node.database.write(prepared.bytes, move |transaction| prepared.commit(transaction)).await.map_err(failure)?;
    // No snapshot is needed for an unopened input. Its text otherwise remains
    // resident in every idle conversation after a first upgrade.
    storage::forget_file(node, id);
    finished(node, id);
    Ok(true)
}
