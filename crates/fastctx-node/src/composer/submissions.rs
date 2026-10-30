//! One composer input per host submission and per natural completion.

use super::*;

pub(super) async fn dispatch(node: &Arc<Node>, id: &ResidentId, flight: Flight) -> Result<Option<Landed>, Failure> {
    let result = async {
        let mut delivery = flight.delivery.clone();
        if let fastctx_types::residents::Notice::Attached { message, .. } = &mut delivery.notice {
            crate::attachments::resolve(node, id, &message.attachments).await?;
            message.attachments =
                crate::attachments::place(node, id, std::mem::take(&mut message.attachments), flight.files_on.as_ref())
                    .await?;
        }
        crate::speech::send_composer(node, id, delivery, flight.idle_only).await
    }
    .await;
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    if next.sending.as_ref().is_none_or(|f| f.delivery.id != flight.delivery.id) {
        return Err(failure("the delivery reservation changed"));
    }
    next.sending = None;
    match &result {
        Ok(None) => {
            // A competing turn started after reservation. The message remains queued.
            next.opportunity = None;
        }
        Ok(Some(Landed::NewTurn { turn } | Landed::JoinedTurn { turn })) => {
            next.queue.retain(|entry| entry.id != flight.entry);
            if flight.automatic && !receipts::ended(node, id, &next, turn).await? {
                next.awaiting_turn = Some(turn.clone());
            }
            if !flight.automatic {
                next.held = None;
            }
        }
        Ok(Some(Landed::Handed { .. })) | Ok(Some(Landed::Waiting { answered: true, .. })) => {
            let already_bound = !next.queue.iter().any(|entry| entry.id == flight.entry);
            if !already_bound {
                next.pending.push(flight.clone());
            }
            if let Ok(Some(Landed::Waiting { why: Some(why), .. })) = &result {
                next.held = Some(why.clone());
            }
        }
        Ok(Some(Landed::Uncertain { why })) => {
            next.held = Some(why.clone());
            if let Some(entry) = next.queue.iter_mut().find(|e| e.id == flight.entry) {
                entry.submitted = Some(flight.delivery.id.clone());
                entry.error = Some(why.clone());
            }
            next.retire_unobserved(&flight.entry);
        }
        Ok(Some(landed)) => {
            let why = format!("the World cannot observe the end of this message's turn: {landed:?}");
            next.held = Some(why.clone());
            if let Some(entry) = next.queue.iter_mut().find(|e| e.id == flight.entry) {
                entry.submitted = Some(flight.delivery.id.clone());
                entry.error = Some(why);
            }
            next.retire_unobserved(&flight.entry);
        }
        // Words sent straight on, the agent idle, never waited: ones that
        // could not go come back to the input rather than into the queue,
        // and the reason goes to the device that sent them.
        Err(_) if flight.straight() => {
            if let Some(position) = next.queue.iter().position(|e| e.id == flight.entry) {
                let entry = next.queue.remove(position);
                next.draft.take_back(entry.document, crate::speech::new_id("back").as_str())?;
            }
        }
        Err(error) => {
            if let Some(entry) = next.queue.iter_mut().find(|e| e.id == flight.entry) {
                entry.error = Some(error.message.clone());
            }
            next.held = Some(error.message.clone());
        }
    }
    if let Some(operation) = &flight.operation {
        let mut receipt = receipts::receipt(node, id, &next, operation)
            .await?
            .ok_or_else(|| failure("the delivery's operation receipt is missing"))?;
        receipt.landed = result.as_ref().ok().cloned().flatten();
        receipt.delivery = Some(flight.delivery.id.clone());
        receipt.failure = result.as_ref().err().map(|error| error.message.clone());
        next.receipts.insert(operation.clone(), receipt);
    }
    publish(node, id, &mut cell, next).await?;
    drop(cell);
    schedule(node, id);
    result
}

/// Associates asynchronous host acceptance with its durable composer reservation.
pub async fn bound(
    node: &Arc<Node>,
    id: &ResidentId,
    delivery: &fastctx_types::NoticeId,
    turn: &str,
) -> Result<(), Failure> {
    let Some(cell) = storage::existing(node, id).await? else {
        return Ok(());
    };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    let flight = next.sending.iter().chain(next.pending.iter()).find(|f| &f.delivery.id == delivery).cloned();
    let Some(flight) = flight else { return Ok(()) };
    next.queue.retain(|entry| entry.id != flight.entry);
    next.pending.retain(|flight| &flight.delivery.id != delivery);
    if flight.automatic && !receipts::ended(node, id, &next, turn).await? {
        next.awaiting_turn = Some(turn.into());
    }
    next.held = None;
    publish(node, id, &mut cell, next).await?;
    drop(cell);
    schedule(node, id);
    Ok(())
}

pub async fn uncertain(
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
    let flight = next.sending.iter().chain(next.pending.iter()).find(|flight| &flight.delivery.id == delivery).cloned();
    let Some(flight) = flight else { return Ok(()) };
    next.pending.retain(|flight| &flight.delivery.id != delivery);
    next.held = Some(why.into());
    if let Some(entry) = next.queue.iter_mut().find(|entry| entry.id == flight.entry) {
        entry.submitted = Some(delivery.clone());
        entry.error = Some(why.into());
    }
    next.retire_unobserved(&flight.entry);
    publish(node, id, &mut cell, next).await
}

pub async fn awaits_turn(node: &Arc<Node>, id: &ResidentId, turn: &str) -> Result<bool, Failure> {
    let Some(cell) = storage::existing(node, id).await? else {
        return Ok(false);
    };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    Ok(cell.as_ref().expect("loaded").awaiting_turn.as_deref() == Some(turn))
}

pub async fn turn_ended(
    node: &Arc<Node>,
    id: &ResidentId,
    turn: &str,
    how: &TurnHow,
    settles_queue: bool,
    order: u64,
) -> Result<(), Failure> {
    let Some(cell) = storage::existing(node, id).await? else {
        return Ok(());
    };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    if receipts::ended(node, id, &next, turn).await? {
        return Ok(());
    }
    next.ended.insert(turn.into());
    let awaited = next.awaiting_turn.as_deref() == Some(turn);
    if awaited {
        next.awaiting_turn = None;
    }
    // Retrying an older terminal outcome must not hold the current queue or
    // replace a later turn's opportunity to release it.
    let later_turn = node.residents.now(id).and_then(|now| now.turn).is_some_and(|active| active != turn);
    let newer = order > next.settled_through || (order == 0 && next.settled_through == 0);
    if settles_queue && newer {
        next.settled_through = order;
    }
    if settles_queue && newer && !later_turn {
        if *how == TurnHow::Completed {
            next.opportunity = Some(turn.into());
        } else {
            next.opportunity = None;
            if next.queue.iter().any(|entry| !entry.command) {
                next.held = Some(format!("turn {how}; queued messages wait for the user"));
            }
        }
    }
    publish(node, id, &mut cell, next).await?;
    drop(cell);
    schedule(node, id);
    Ok(())
}

pub(super) fn schedule(node: &Arc<Node>, id: &ResidentId) {
    if !node.effects_ready.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    // A control waiting behind these words may go once they have gone or
    // are held.
    crate::control_queue::poke(node, id);
    {
        let mut releases = node.composers.releases.lock().expect("composer releases");
        if let Some(again) = releases.get_mut(id) {
            *again = true;
            return;
        }
        releases.insert(id.clone(), false);
    }
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
        loop {
            if let Err(error) = release(&node, &id).await {
                node.log.line(format_args!("{id}'s deferred queue: {error}"));
            }
            let mut releases = node.composers.releases.lock().expect("composer releases");
            if releases.get_mut(&id).is_some_and(std::mem::take) {
                continue;
            }
            releases.remove(&id);
            break;
        }
    });
}

pub fn resume(node: &Arc<Node>) {
    super::recovery::migrate(node);
}

pub(super) fn after_commit(
    node: &Arc<Node>,
    id: &ResidentId,
    flight: Flight,
    reply: Option<tokio::sync::oneshot::Sender<Result<Option<Landed>, Failure>>>,
) -> impl FnOnce() + Send + 'static {
    let (node, id, runtime) = (node.clone(), id.clone(), tokio::runtime::Handle::current());
    move || {
        let work: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> = Box::pin(async move {
            let result = dispatch(&node, &id, flight).await;
            if let Some(reply) = reply {
                let _ = reply.send(result);
            } else if let Err(error) = result {
                node.log.line(format_args!("{id}'s committed input: {error}"));
            }
        });
        runtime.spawn(work);
    }
}

async fn release(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    node.ready().await;
    // Removed, being deleted or deleted: what is queued for it waits, or
    // goes with it.
    if node.residents.active(id).is_err() {
        return Ok(());
    }
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    // A command's place whose command is gone goes with it; one still
    // waiting at the head goes before the words.
    if commands::settle(node, id, &mut cell).await? {
        crate::control_queue::poke(node, id);
        return Ok(());
    }
    let state = cell.as_ref().expect("loaded");
    let now = node.residents.now(id).ok_or_else(|| failure("unknown resident"))?;
    if state.opportunity.is_none()
        || state.held.is_some()
        || state.awaiting_turn.is_some()
        || state.sending.is_some()
        || !state.pending.is_empty()
        || now.activity == Activity::Busy
        || now.waiting_for_user
        || state.queue.first().is_none_or(|e| !e.editors.is_empty() || !e.document.uploads.is_empty())
    {
        return Ok(());
    }
    let mut next = cell.take().expect("loaded");
    let entry = next.queue[0].id.clone();
    let flight = next.reserve(&entry, true, true)?;
    storage::publish_then(node, id, &mut cell, next, after_commit(node, id, flight, None)).await
}
