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

pub(super) fn load<'a>(node: &'a Arc<Node>, id: &'a ResidentId, cell: &'a mut Option<Stored>) -> Future<'a, ()> {
    Box::pin(async move {
        let recover = super::recovery::pending(node, id);
        if cell.is_some() && !recover {
            return Ok(());
        }
        if !node.residents.knows(id) {
            return Err(failure("unknown resident"));
        }
        let mut state = match cell.take() {
            Some(state) => state,
            None => persistence::read(node, id).await?,
        };
        let mut changed = false;
        if recover {
            changed |= super::recovery::settle(node, id, &mut state).await?;
            changed |= uploads::recover(node, id, &mut state).await?;
        }
        if !state.persisted || changed {
            let prepared = persistence::prepare(id, &mut state)?;
            node.database
                .write(prepared.bytes, move |transaction| prepared.commit(transaction))
                .await
                .map_err(failure)?;
        }
        // The draft file an older node kept goes once the database holds
        // the draft; where removing it fails, the database is read first.
        forget_file(node, id);
        if recover {
            super::recovery::finished(node, id);
        }
        watching::restore(node, id, &mut state);
        failures::apply(node, id, &mut state);
        let view = state.view();
        let busy = super::idle::busy(&state);
        *cell = Some(state);
        if recover {
            publication::committed(node, &tokio::runtime::Handle::current(), id, Arc::new(view), busy);
        } else {
            cache(node, id, view);
        }
        Ok(())
    })
}

/// Takes away the draft file an older node kept, the database now holding
/// the draft.
pub(super) fn forget_file(node: &Node, id: &ResidentId) {
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

/// Marks inputs for lazy recovery after startup has restored host facts.
/// Registering identities does not read or materialize any draft.
pub fn prepare_recovery(node: &Node) {
    let ids: Vec<_> =
        node.residents.everyone().filter(|resident| resident.removed.is_none()).map(|resident| resident.id).collect();
    node.composers.recovering.lock().expect("composer recovery").extend(ids);
}

/// Completes this conversation's old reservation before any inbox batch can
/// reach a host. The caller has not acquired the conversation's action gate.
pub(crate) async fn restore_before_delivery(node: &Arc<Node>, id: &ResidentId) -> Result<bool, Failure> {
    if !super::recovery::pending(node, id) {
        return Ok(false);
    }
    let gate = node.residents.management(id)?;
    let _reading = crate::gate_waits::shared(node, id, &gate).await?;
    node.residents.active(id)?;
    if let Some(cell) = existing(node, id).await? {
        let mut cell = cell.lock().await;
        load(node, id, &mut cell).await?;
    } else {
        super::recovery::finished(node, id);
    }
    Ok(true)
}

pub(super) async fn recover_one(node: &Arc<Node>, resident: &ResidentId) -> Result<(), Failure> {
    let gate = node.residents.management(resident)?;
    let _reading = gate.read().await;
    node.residents.active(resident)?;
    let Some(cell) = existing(node, resident).await? else {
        return Ok(());
    };
    let mut cell = cell.lock().await;
    load(node, resident, &mut cell).await?;
    drop(cell);
    schedule(node, resident);
    Ok(())
}
