//! The only writer and release coordinator of each conversation's composer.

mod commands;
mod idle;
pub(crate) use idle::input_busy;
mod receipts;
mod rejected;
pub(crate) use rejected::rejected;
mod state;
mod storage;
mod submissions;
pub(crate) mod uploads;
mod watching;

pub use commands::{forget_command, next_command, place_command};
use fastctx_types::access::Written;
pub use storage::recover;
use storage::{event, load, publish};
pub use submissions::resume;
pub use submissions::{awaits_turn, bound, turn_ended, uncertain};
use submissions::{dispatch, schedule};
pub use watching::{Lease, snapshot, watch};

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::composer::{Change, Request, Updated};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Activity, Landed};
use fastctx_types::session::{Happened, SessionChange, SessionEvent, TurnHow};
use fastctx_types::{MachineName, ResidentId};

use crate::Node;
use state::{Entry, Flight, Receipt, Stored};

type Cell = crate::leases::Lease<ResidentId, fastctx_platform::doing::Mutex<Option<Stored>>>;

#[derive(Default)]
pub struct Composers {
    cells: crate::leases::Registry<ResidentId, fastctx_platform::doing::Mutex<Option<Stored>>>,
    watches: Mutex<HashMap<String, ResidentId>>,
    failed: Mutex<HashMap<ResidentId, Cell>>,
}

pub fn removed(node: &Node, id: &ResidentId) {
    node.composers.failed.lock().expect("failed composer checkpoints").remove(id);
}

/// Forgets everything held in memory of the composer of `id`, whose
/// conversation is deleted: its state, and the watches registered for it.
pub fn deleted(node: &Node, id: &ResidentId) {
    removed(node, id);
    node.composers.watches.lock().expect("composer watches").retain(|_, watched| watched != id);
    if let Some(cell) = node.composers.cells.get(id) {
        tokio::spawn(async move { *cell.lock().await = None });
    }
}

impl Composers {
    fn cell(&self, id: &ResidentId) -> Cell {
        self.cells.get_or_insert_with(id, || crate::ranked::composer(id))
    }
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("composer: {error}"))
}

pub async fn request(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
    request: Request,
) -> Result<Updated, Failure> {
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.about(&resident).is_some() => node.machine().into(),
        None => crate::colleagues::find_for(node, &Acting::Here, &resident).await?,
    };
    if local || node.is_here(&machine) {
        return here(node, &node.machine(), &resident, request).await;
    }
    let call = RemoteCall::Composer { resident, request };
    match node
        .call(&Acting::Here, &machine, Purpose::Agents, call)
        .await
        .map_err(|error| Failure::unanswered(error.to_string()))?
    {
        Ok(RemoteReply::Composer(updated)) => Ok(*updated),
        Ok(_) => Err(failure("the machine answered something other than a composer")),
        Err(error) => Err(error),
    }
}

pub async fn here(node: &Arc<Node>, from: &MachineName, id: &ResidentId, request: Request) -> Result<Updated, Failure> {
    node.ready().await;
    let gate = node.residents.management(id)?;
    let _reading = crate::gate_waits::shared(node, id, &gate).await?;
    node.residents.active(id)?;
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell)?;
    if matches!(request, Request::Read) {
        return Ok(Updated { composer: cell.as_ref().expect("loaded").view(), landed: None, delivery: None });
    }
    let Request::Mutate { mutation } = request else { unreachable!() };
    let mutation = *mutation;
    if mutation.operation.is_empty() {
        return Err(failure("an operation ID is required"));
    }
    let state = cell.as_ref().expect("loaded");
    if receipts::upload(node, id, state, &mutation.operation)?.is_some() {
        return Err(failure("that operation ID already belongs to an upload"));
    }
    if let Some(receipt) = receipts::receipt(node, id, state, &mutation.operation)? {
        if receipt.from != *from || receipt.mutation != mutation {
            return Err(failure("an operation ID was reused for different content or a different device"));
        }
        if let Some(error) = &receipt.failure {
            return Err(Failure::new(error.clone()));
        }
        return Ok(Updated {
            composer: state.view(),
            landed: receipt.landed.clone(),
            delivery: receipt.delivery.clone(),
        });
    }
    if let Change::Attach { attachment, .. } = &mutation.change {
        crate::attachments::resolve(node, id, std::slice::from_ref(attachment)).await?;
    }
    let mut next = state.clone();
    let mut flight = None;
    match &mutation.change {
        Change::Send { epoch } => {
            if next.draft.epoch != *epoch {
                return Err(failure("this draft was already sent"));
            }
            let draft = next.draft.view();
            if !draft.uploads.is_empty() {
                return Err(failure(
                    "the shared draft has unfinished attachment uploads; wait for them or remove them",
                ));
            }
            if draft.text.trim().is_empty() && draft.attachments.is_empty() {
                return Err(failure("the shared draft is empty"));
            }
            let entry = crate::speech::new_id("queued");
            let document = std::mem::take(&mut next.draft);
            next.draft.epoch = epoch.checked_add(1).ok_or_else(|| failure("draft epoch overflow"))?;
            next.queue.push(Entry {
                id: entry.clone(),
                from: from.clone(),
                document,
                editors: HashSet::new(),
                error: None,
                submitted: None,
                command: false,
            });
            let now = node.residents.now(id).ok_or_else(|| failure("unknown resident"))?;
            if now.activity != Activity::Busy
                && next.awaiting_turn.is_none()
                && next.sending.is_none()
                && next.pending.is_empty()
            {
                flight = Some(next.reserve(&entry, false, true)?);
            }
        }
        Change::SendQueued { id } => flight = Some(next.reserve(id, false, false)?),
        Change::Editing { id: entry, watch, editing } => {
            if node.composers.watches.lock().expect("composer watches").get(watch) != Some(id) {
                return Err(failure("the composer watch is no longer open"));
            }
            let entry = next.entry(entry)?;
            if *editing {
                entry.editors.insert(watch.clone());
            } else {
                entry.editors.remove(watch);
            }
        }
        _ => next.edit(&mutation)?,
    }
    next.receipts.insert(
        mutation.operation.clone(),
        Receipt { from: from.clone(), mutation: mutation.clone(), landed: None, delivery: None, failure: None },
    );
    let forsaken = uploads::forsaken(state, &next);
    publish(node, id, &mut cell, next)?;
    crate::attachments::forsake(node, id, &forsaken);
    let view = cell.as_ref().expect("loaded").view();
    drop(cell);
    drop(_reading);
    let Some(flight) = flight else {
        schedule(node, id);
        return Ok(Updated { composer: view, landed: None, delivery: None });
    };
    let delivery = flight.delivery.id.clone();
    let (sending_node, sending_id) = (node.clone(), id.clone());
    let delivered =
        tokio::spawn(async move { dispatch(&sending_node, &sending_id, flight).await }).await.map_err(failure)?;
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell)?;
    let mut next = cell.as_ref().expect("loaded").clone();
    let receipt = receipts::receipt(node, id, &next, &mutation.operation)?.expect("kept receipt");
    let receipt = next.receipts.entry(mutation.operation.clone()).or_insert(receipt);
    receipt.landed = delivered.as_ref().ok().cloned().flatten();
    receipt.delivery = Some(delivery.clone());
    receipt.failure = delivered.as_ref().err().map(|error| error.message.clone());
    publish(node, id, &mut cell, next)?;
    let composer = cell.as_ref().expect("loaded").view();
    drop(cell);
    schedule(node, id);
    delivered.map(|landed| Updated { composer, landed, delivery: Some(delivery) })
}

pub fn changed(node: &Arc<Node>, id: &ResidentId) {
    schedule(node, id);
}
