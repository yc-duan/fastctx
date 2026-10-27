//! Durable composer state. Mutations own their document until commit; errors
//! leave the cell empty so the next caller reconstructs confirmed authority.

use super::*;

pub(super) fn existing<'a>(node: &'a Node, id: &'a ResidentId) -> Future<'a, Option<Cell>> {
    Box::pin(async move {
        if !node.residents.knows(id) {
            return Ok(None);
        }
        if let Some(cell) = node.composers.cells.get(id) {
            return Ok(Some(cell));
        }
        if persistence::exists(node, id).await? {
            return Ok(Some(node.composers.cell(id)));
        }
        let directory = node.world.conversation(id);
        let exists = tokio::task::spawn_blocking(move || {
            let _transition = crate::resident_storage::lock();
            directory.composer().try_exists()
        })
        .await
        .map_err(failure)?
        .map_err(failure)?;
        Ok(exists.then(|| node.composers.cell(id)))
    })
}

pub(super) fn load<'a>(node: &'a Node, id: &'a ResidentId, cell: &'a mut Option<Stored>) -> Future<'a, ()> {
    Box::pin(async move {
        if cell.is_some() {
            return Ok(());
        }
        if !node.residents.knows(id) {
            return Err(failure("unknown resident"));
        }
        let mut state = persistence::read(node, id).await?;
        if !state.persisted {
            let prepared = persistence::prepare(id, &mut state)?;
            node.database
                .write(prepared.bytes, move |transaction| prepared.commit(transaction))
                .await
                .map_err(failure)?;
        }
        // The draft file an older node kept goes once the database holds
        // the draft; where removing it fails, the database is read first.
        forget_file(node, id);
        watching::restore(node, id, &mut state);
        failures::apply(node, id, &mut state);
        cache(node, id, state.view());
        *cell = Some(state);
        Ok(())
    })
}

/// Takes away the draft file an older node kept, the database now holding
/// the draft.
fn forget_file(node: &Node, id: &ResidentId) {
    let file = node.world.conversation(id).composer();
    let _transition = crate::resident_storage::lock();
    match std::fs::remove_file(&file) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => node.log.line(format_args!("taking away {}: {error}", file.display())),
    }
}

pub(super) fn publish<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
    cell: &'a mut Option<Stored>,
    next: Stored,
) -> Future<'a, ()> {
    Box::pin(async move { publish_then(node, id, cell, next, || {}).await })
}

pub(super) fn publish_then<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
    cell: &'a mut Option<Stored>,
    mut next: Stored,
    committed: impl FnOnce() + Send + 'static,
) -> Future<'a, ()> {
    Box::pin(async move {
        if !node.residents.knows(id) {
            return Err(failure("unknown resident"));
        }
        let prepared = persistence::prepare(id, &mut next)?;
        let documents = node.composers.streams.prepare(id, synchronized(&mut next, &prepared)?);
        let view = Arc::new(next.view());
        let busy = super::idle::busy(&next);
        let (weak, published_id, runtime) = (Arc::downgrade(node), id.clone(), tokio::runtime::Handle::current());
        let result = node
            .database
            .write_then(
                prepared.bytes,
                move |transaction| prepared.commit(transaction),
                move |_| {
                    documents.committed();
                    if let Some(node) = weak.upgrade() {
                        publication::committed(&node, &runtime, &published_id, view, busy);
                    }
                    committed();
                },
            )
            .await;
        match result {
            Ok(()) => {
                *cell = Some(next);
                Ok(())
            }
            Err(reason) => {
                let error = failure(reason);
                failures::hold(node, id, next.revision, next.held.unwrap_or_else(|| error.message.clone()));
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
                Err(error)
            }
        }
    })
}

fn synchronized(
    state: &mut Stored,
    prepared: &persistence::Prepared,
) -> Result<fastctx_types::composer::sync::Frame, Failure> {
    use fastctx_types::composer::sync::{self, Bytes, Frame, Update};
    let visible = state.visible_document_ids();
    let mut documents = prepared.updates()?;
    documents.retain(|document| visible.contains(&document.document));
    let provided: HashSet<_> = documents.iter().map(|document| document.document.clone()).collect();
    for document in state.documents().filter(|document| {
        visible.contains(&document.id)
            && !state.observed_documents.contains(&document.id)
            && !provided.contains(&document.id)
    }) {
        documents.push(Update {
            document: document.id.clone(),
            revision: document.revision,
            update: Bytes(document.update_for(None)?),
        });
    }
    state.observed_documents = visible;
    Ok(Frame::State { version: sync::VERSION, state: Box::new(state.metadata()), documents })
}

pub(super) fn cache(node: &Node, id: &ResidentId, composer: fastctx_types::composer::Composer) {
    publication::cache(&node.composers.snapshots, id, Arc::new(composer));
}

pub(super) fn event(composer: fastctx_types::composer::Composer) -> SessionEvent {
    SessionEvent {
        at: jiff::Timestamp::now(),
        happened: Happened::Changed { turn: None, change: SessionChange::Composer { composer: Box::new(composer) } },
    }
}

/// Restores delivery reservations before automatic release starts. A
/// conversation whose input cannot be restored is told in the log and left
/// for the next start; the others, and the node, go on
/// (`specs/world-concurrency.md`, 多步的事: 收尾从不挡节点起来).
pub async fn recover(node: &Arc<Node>) {
    for resident in node.residents.everyone() {
        if resident.removed.is_some() {
            continue;
        }
        if let Err(failure) = recover_one(node, &resident.id).await {
            node.log.line(format_args!("restoring the shared input of {}: {}", resident.id, failure.message));
        }
    }
}

async fn recover_one(node: &Arc<Node>, resident: &ResidentId) -> Result<(), Failure> {
    let directory = node.world.conversation(resident);
    let Some(cell) = existing(node, resident).await? else {
        return Ok(());
    };
    let mut cell = cell.lock().await;
    load(node, resident, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    let flights: Vec<_> = next.sending.take().into_iter().chain(next.pending.clone()).collect();
    for flight in flights {
        if let Some(turn) = crate::delivery::input_turn(&directory, &flight.delivery.id).map_err(failure)? {
            next.queue.retain(|entry| entry.id != flight.entry);
            next.pending.retain(|pending| pending.delivery.id != flight.delivery.id);
            if flight.automatic && !receipts::ended(node, resident, &next, &turn).await? {
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
                let kept = crate::inbox::read(&directory, &flight.delivery.id).map_err(failure)?;
                if kept.as_ref().is_some_and(|kept| kept.state != fastctx_types::residents::PendingState::Waiting) {
                    // On its way to the host when the node stopped: whether
                    // it went in is for the delivery's own recovery to say
                    // (`speech::recover`), which tells the input as it does.
                    next.pending.push(flight);
                    continue;
                }
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
    publish(node, resident, &mut cell, next).await?;
    drop(cell);
    uploads::recover(node, resident).await?;
    schedule(node, resident);
    Ok(())
}
