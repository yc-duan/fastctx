//! Native document synchronization. Text-only transactions neither rebuild the
//! queue nor serialize a whole legacy composer unless an old viewer needs it.

mod source;
use super::*;
use fastctx_net::watch_queue;
use fastctx_types::composer::sync::{self, Bytes, Frame, Input, Subscription, Update};
pub(super) use source::Sources;

pub struct Documents {
    source: Arc<source::Shared>,
    name: String,
    events: watch_queue::Receiver<Arc<Frame>>,
    initial: Option<Arc<Frame>>,
    through: u64,
    _initial_memory: Option<fastctx_net::reassembly::Reservation>,
    _lease: Lease,
}
impl Documents {
    pub async fn next(&mut self) -> Result<Option<Arc<Frame>>, Failure> {
        if let Some(initial) = self.initial.take() {
            return Ok(Some(initial));
        }
        self._initial_memory = None;
        loop {
            let Some(frame) = self.events.recv().await.map_err(failure)? else { return Ok(None) };
            let revision = match frame.as_ref() {
                Frame::State { state, .. } => state.revision,
                Frame::Update { revision, .. } => *revision,
                _ => unreachable!("acknowledgements are private to the input subscription"),
            };
            if revision <= self.through {
                continue;
            }
            self.through = revision;
            return Ok(Some(frame));
        }
    }
}
impl Drop for Documents {
    fn drop(&mut self) {
        self.source.remove(&self.name);
    }
}

/// Called after the ordinary conversation watch authorization. The lease keeps
/// queued editing scoped to this subscription, just as with the legacy watch.
pub fn watch_documents<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
    name: String,
    subscription: Subscription,
) -> Future<'a, Documents> {
    Box::pin(async move {
        if subscription.version != sync::VERSION {
            return Err(failure("this document synchronization version is not supported"));
        }
        let lease = watching::register(node, id, name.clone(), false)?;
        let source = node.composers.streams.open(id);
        let events = source.subscribe(&name);
        // Construct the owner before awaits so every failure unregisters it.
        let mut watching =
            Documents { source, name, events, initial: None, through: 0, _initial_memory: None, _lease: lease };
        let gate = node.residents.management(id)?;
        let _reading = crate::gate_waits::shared(node, id, &gate).await?;
        node.residents.active(id)?;
        let cell = node.composers.cell(id);
        let mut cell = cell.lock().await;
        load(node, id, &mut cell).await?;
        let state = cell.as_ref().expect("loaded");
        let mut known = HashMap::new();
        for vector in subscription.known {
            if known.insert(vector.document, vector.vector).is_some() {
                return Err(failure("the document state vector was repeated"));
            }
        }
        let metadata = state.metadata();
        let visible = std::iter::once(&metadata.draft)
            .chain(metadata.queue.iter().chain(&metadata.unobserved).map(|entry| &entry.document))
            .map(|document| document.id.as_str())
            .collect::<HashSet<_>>();
        let documents = state
            .documents()
            .filter(|document| visible.contains(document.id.as_str()))
            .map(|document| {
                Ok(Update {
                    document: document.id.clone(),
                    revision: document.revision,
                    update: Bytes(document.update_for(known.get(&document.id).map(|bytes| bytes.0.as_slice()))?),
                })
            })
            .collect::<Result<Vec<_>, Failure>>()?;
        watching.through = state.revision;
        let initial = Frame::State { version: sync::VERSION, state: Box::new(metadata), documents };
        watching._initial_memory =
            Some(fastctx_net::reassembly::Reservation::new(watch_queue::size(&initial)).map_err(failure)?);
        watching.initial = Some(Arc::new(initial));
        Ok(watching)
    })
}

/// The enclosing stream admits and authorizes each input independently. Once
/// accepted, its owner continues this future even when that stream disconnects.
pub fn update_document<'a>(node: &'a Arc<Node>, id: &'a ResidentId, input: Input) -> Future<'a, Frame> {
    Box::pin(async move {
        node.ready().await;
        let Input::Update { number, document, update } = input;
        let gate = node.residents.management(id)?;
        let _reading = crate::gate_waits::shared(node, id, &gate).await?;
        node.residents.active(id)?;
        let cell = node.composers.cell(id);
        let mut cell = cell.lock().await;
        load(node, id, &mut cell).await?;
        let mut next = cell.take().expect("loaded");
        next.editable_document(&document)?.merge(&update.0)?;
        // An earlier failed stop is first consolidated with the metadata. This
        // exceptional write must not let subsequent typing erase its hold.
        let held = node.composers.holds.lock().expect("composer checkpoint failures").contains_key(id);
        if held {
            publish(node, id, &mut cell, next).await?;
            let state = cell.as_mut().expect("committed");
            let revision = state.revision;
            return Ok(Frame::Saved {
                number,
                document_revision: state.editable_document(&document)?.revision,
                document,
                revision,
            });
        }
        let prepared = persistence::prepare_text(id, &mut next, &document)?;
        let changed = prepared.update()?;
        let document_revision = changed.revision;
        let revision = next.revision;
        let publication = node.composers.streams.prepare(id, Frame::Update { revision, document: changed });
        let legacy = watching::legacy(node, id).then(|| Arc::new(next.view()));
        let busy = idle::busy(&next);
        let (committed_node, committed_id, runtime) = (node.clone(), id.clone(), tokio::runtime::Handle::current());
        let result = node
            .database
            .write_then(
                prepared.bytes,
                move |transaction| prepared.commit(transaction),
                move |_| {
                    publication.committed();
                    match legacy {
                        Some(view) => publication::committed(&committed_node, &runtime, &committed_id, view, busy),
                        None => publication::invalidate(&committed_node, &committed_id, revision),
                    }
                },
            )
            .await;
        match result {
            Ok(()) => {
                *cell = Some(next);
                Ok(Frame::Saved { number, document, revision, document_revision })
            }
            Err(error) => {
                failures::hold(node, id, revision, next.held.unwrap_or_else(|| error.to_string()));
                Err(failure(error))
            }
        }
    })
}
