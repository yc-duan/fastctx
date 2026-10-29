//! The only writer and release coordinator of each conversation's composer.

mod commands;
mod document;
mod failures;
mod idle;
mod keeping;
pub(crate) use idle::input_busy;
pub(crate) use keeping::start as keep;
mod persistence;
mod publication;
mod receipts;
pub(crate) use persistence::SCHEMA;
mod rejected;
pub(crate) use rejected::rejected;
mod sending;
mod state;
mod storage;
mod submissions;
mod synchronizing;
pub use synchronizing::{update_document, watch_documents};
pub(crate) mod uploads;
mod watching;

pub use commands::{forget_command, next_command, place_command};
use fastctx_types::access::Written;
pub use storage::recover;
use storage::{event, load, publish};
pub use submissions::resume;
use submissions::schedule;
pub use submissions::{awaits_turn, bound, turn_ended, uncertain};
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
// These entry points also sit deep in host-report and lifecycle futures.
// Keep their transaction state off those callers' stacks.
type Future<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, Failure>> + Send + 'a>>;

#[derive(Default)]
pub struct Composers {
    cells: crate::leases::Registry<ResidentId, fastctx_platform::doing::Mutex<Option<Stored>>>,
    watches: Mutex<HashMap<String, ResidentId>>,
    legacy_watchers: Mutex<HashMap<ResidentId, usize>>,
    streams: synchronizing::Sources,
    editors: Mutex<HashMap<ResidentId, HashMap<String, HashSet<fastctx_types::NoticeId>>>>,
    publications: publication::Publications,
    holds: failures::Holds,
    releases: Mutex<HashMap<ResidentId, bool>>,
    upload_failures: Arc<Mutex<HashMap<ResidentId, HashMap<String, String>>>>,
    snapshots: publication::Snapshots,
}

pub fn removed(node: &Node, id: &ResidentId) {
    node.composers.snapshots.lock().expect("composer snapshots").remove(id);
}

/// Forgets everything held in memory of the composer of `id`, whose
/// conversation is deleted: its state, and the watches registered for it.
pub async fn deleted(node: &Node, id: &ResidentId) -> Result<(), Failure> {
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    persistence::delete(node, id).await?;
    *cell = None;
    removed(node, id);
    node.composers.watches.lock().expect("composer watches").retain(|_, watched| watched != id);
    node.composers.editors.lock().expect("composer editors").remove(id);
    node.composers.upload_failures.lock().expect("composer upload failures").remove(id);
    node.composers.holds.lock().expect("composer checkpoint failures").remove(id);
    Ok(())
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

pub fn here<'a>(
    node: &'a Arc<Node>,
    from: &'a MachineName,
    id: &'a ResidentId,
    request: Request,
) -> Future<'a, Updated> {
    Box::pin(async move {
        node.ready().await;
        if let Request::Sync { input } = request {
            update_document(node, id, *input).await?;
            return here(node, from, id, Request::Read).await;
        }
        if matches!(request, Request::Read) {
            node.residents.active(id)?;
            let snapshot = node
                .composers
                .snapshots
                .lock()
                .expect("composer snapshots")
                .get(id)
                .and_then(|snapshot| snapshot.view.clone());
            if let Some(composer) = snapshot {
                return Ok(Updated { composer: (*composer).clone(), landed: None, delivery: None });
            }
            let gate = node.residents.management(id)?;
            let _reading = crate::gate_waits::shared(node, id, &gate).await?;
            node.residents.active(id)?;
            let cell = node.composers.cell(id);
            let mut cell = cell.lock().await;
            load(node, id, &mut cell).await?;
            return Ok(Updated { composer: cell.as_ref().expect("loaded").view(), landed: None, delivery: None });
        }
        let gate = node.residents.management(id)?;
        let _reading = crate::gate_waits::shared(node, id, &gate).await?;
        node.residents.active(id)?;
        let cell = node.composers.cell(id);
        let mut cell = cell.lock().await;
        load(node, id, &mut cell).await?;

        let Request::Mutate { mutation } = request else { unreachable!() };
        let mutation = *mutation;
        if mutation.operation.is_empty() {
            return Err(failure("an operation ID is required"));
        }
        let state = cell.as_ref().expect("loaded");
        if receipts::upload(node, id, state, &mutation.operation).await?.is_some() {
            return Err(failure("that operation ID already belongs to an upload"));
        }
        if let Some(receipt) = receipts::receipt(node, id, state, &mutation.operation).await? {
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
        // An edit of an epoch of the draft sent since, from a device whose
        // answer and receipt are gone, was done if that epoch took it: it is
        // not done again in the new one (review 2.1).
        if let Change::Edit { target: fastctx_types::composer::Target::Draft, epoch, .. } = &mutation.change
            && *epoch < state.draft.epoch
            && receipts::taken(node, id, state, *epoch, &mutation.operation).await?
        {
            return Ok(Updated { composer: state.view(), landed: None, delivery: None });
        }
        if let Change::Attach { attachment, .. } = &mutation.change {
            crate::attachments::resolve(node, id, std::slice::from_ref(attachment)).await?;
        }
        let was_uploaded = uploads::listed(state);
        let mut next = cell.take().expect("loaded");
        let mut flight = None;
        match &mutation.change {
            Change::Send { .. } | Change::SendDocument { .. } => {
                flight = sending::capture(node, id, &mut next, from, &mutation.change)?;
            }
            Change::SendQueued { id } => flight = Some(next.reserve(id, false, false)?),
            Change::Editing { id: entry, watch, editing } => {
                if node.composers.watches.lock().expect("composer watches").get(watch) != Some(id) {
                    return Err(failure("the composer watch is no longer open"));
                }
                let entry = next.entry(entry)?;
                watching::editing(node, id, watch, &entry.id, *editing);
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
        let forsaken = uploads::forsaken(was_uploaded, &next);
        let sending = flight.map(|mut flight| {
            flight.operation = Some(mutation.operation.clone());
            next.sending = Some(flight.clone());
            let delivery = flight.delivery.id.clone();
            let (reply, answer) = tokio::sync::oneshot::channel();
            (flight, delivery, reply, answer)
        });
        let (callback, answer) = match sending {
            Some((flight, delivery, reply, answer)) => {
                (Some(submissions::after_commit(node, id, flight, Some(reply))), Some((delivery, answer)))
            }
            None => (None, None),
        };
        let (committed_node, committed_id, runtime) = (node.clone(), id.clone(), tokio::runtime::Handle::current());
        storage::publish_then(node, id, &mut cell, next, move || {
            if let Some(callback) = callback {
                callback();
            }
            runtime.spawn(async move {
                crate::attachments::forsake(&committed_node, &committed_id, &forsaken);
            });
        })
        .await?;
        let view = cell.as_ref().expect("loaded").view();
        drop(cell);
        drop(_reading);
        let Some((delivery, answer)) = answer else {
            schedule(node, id);
            return Ok(Updated { composer: view, landed: None, delivery: None });
        };
        let delivered = answer.await.map_err(failure)?;
        let cell = node.composers.cell(id);
        let mut cell = cell.lock().await;
        load(node, id, &mut cell).await?;
        let composer = cell.as_ref().expect("loaded").view();
        delivered.map(|landed| Updated { composer, landed, delivery: Some(delivery) })
    })
}

pub fn changed(node: &Arc<Node>, id: &ResidentId) {
    schedule(node, id);
}
