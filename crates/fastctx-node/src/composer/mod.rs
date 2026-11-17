//! The only writer and release coordinator of each conversation's composer.

mod answer;
mod capture;
mod commands;
pub(crate) mod compaction;
mod document;
mod durability;
mod failures;
mod idle;
mod keeping;
mod legacy_lined;
mod legacy_records;
mod legacy_refresh;
mod limits;
pub(crate) use idle::input_busy;
pub(crate) use keeping::start as keep;
pub(crate) use limits::update as validate_document_input;
mod persistence;
mod publication;
mod receipts;
mod recovery;
pub(crate) use persistence::SCHEMA;
mod rejected;
pub(crate) use rejected::rejected;
mod sending;
mod settlement;
mod settling;
mod state;
mod storage;
mod submissions;
mod synchronizing;
pub use synchronizing::{update_document, update_document_from, watch_documents};
pub(crate) mod uploads;
mod watching;
mod withdrawing;

pub use commands::{forget_command, next_command, place_command};
use fastctx_types::access::Written;
pub use storage::prepare_recovery;
pub(crate) use storage::restore_before_delivery;
use storage::{event, load, publish};
pub use submissions::resume;
use submissions::schedule;
pub use submissions::{
    awaits_turn, bound, stop_asked, stop_coming, stop_resolved, stopped_turn, turn_ended, uncertain,
};
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
use state::{Entry, Flight, Receipt, StopMark, Stored};

type Cell = crate::leases::Lease<ResidentId, fastctx_platform::doing::Mutex<Option<Stored>>>;
// These entry points also sit deep in host-report and lifecycle futures.
// Keep their transaction state off those callers' stacks.
type Future<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, Failure>> + Send + 'a>>;

#[derive(Default)]
pub struct Composers {
    recovering: Mutex<HashSet<ResidentId>>,
    cells: crate::leases::Registry<ResidentId, fastctx_platform::doing::Mutex<Option<Stored>>>,
    watches: Mutex<HashMap<String, ResidentId>>,
    legacy_watchers: Mutex<HashMap<ResidentId, usize>>,
    streams: synchronizing::Sources,
    editors: Mutex<HashMap<ResidentId, HashMap<String, HashSet<fastctx_types::NoticeId>>>>,
    publications: publication::Publications,
    holds: failures::Holds,
    releases: Mutex<HashMap<ResidentId, bool>>,
    /// The stop the user asked for in each conversation, until it ends
    /// (`submissions::stop_asked`); the composer state keeps it as well.
    stopping: Mutex<HashMap<ResidentId, submissions::Stop>>,
    upload_failures: Arc<Mutex<HashMap<ResidentId, HashMap<String, String>>>>,
    snapshots: publication::Snapshots,
    legacy_refreshes: legacy_refresh::Refreshes,
    durability: Arc<durability::Durability>,
    settling: Arc<settling::Settling>,
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
    recovery::finished(node, id);
    node.composers.watches.lock().expect("composer watches").retain(|_, watched| watched != id);
    node.composers.editors.lock().expect("composer editors").remove(id);
    node.composers.upload_failures.lock().expect("composer upload failures").remove(id);
    node.composers.holds.lock().expect("composer checkpoint failures").remove(id);
    node.composers.stopping.lock().expect("composer stops").remove(id);
    settling::forget(node, id);
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
    // A captured send reaches another machine only by the conversation's
    // document subscription, whose input an older node refuses. As a plain
    // request an older node would take the fresh snapshot for a whole
    // replica and leave the sent words in the draft.
    if let Request::Mutate { mutation } = &request
        && matches!(mutation.change, Change::SendDocument { take: Some(_), .. })
    {
        return Err(Failure::unsupported("a captured send goes by the conversation's document subscription"));
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
    Box::pin(async move { accepting(node, from, id, request, false).await.map(|(answer, _)| answer.updated()) })
}

fn accepting<'a>(
    node: &'a Arc<Node>,
    from: &'a MachineName,
    id: &'a ResidentId,
    request: Request,
    bounded: bool,
) -> Future<'a, (answer::Answer, bool)> {
    Box::pin(async move {
        limits::request(&request)?;
        let captured = capture::prepare(&request)?;
        if let Request::Sync { input } = request {
            update_document_from(node, from, id, *input).await?;
            return here(node, from, id, Request::Read).await.map(|updated| (answer::Answer::legacy(updated), false));
        }
        node.ready_for(id).await;
        if matches!(request, Request::Read) {
            node.residents.active(id)?;
            let snapshot = node
                .composers
                .snapshots
                .lock()
                .expect("composer snapshots")
                .get(id)
                .and_then(|snapshot| snapshot.view.clone());
            if let Some(composer) = snapshot.filter(|_| !recovery::pending(node, id)) {
                return Ok((
                    answer::Answer::legacy(Updated { composer: (*composer).clone(), landed: None, delivery: None }),
                    false,
                ));
            }
            let gate = node.residents.management(id)?;
            let _reading = crate::gate_waits::shared(node, id, &gate).await?;
            node.residents.active(id)?;
            let cell = node.composers.cell(id);
            let mut cell = cell.lock().await;
            load(node, id, &mut cell).await?;
            drop(cell);
            drop(_reading);
            let (composer, _) = legacy_refresh::snapshot(node, id).await?;
            publication::cache(
                &node.composers.snapshots,
                &tokio::runtime::Handle::current(),
                id,
                Arc::new(composer.clone()),
            );
            return Ok((answer::Answer::legacy(Updated { composer, landed: None, delivery: None }), false));
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
            let pending = state
                .sending
                .iter()
                .chain(&state.pending)
                .find(|flight| flight.operation.as_deref() == Some(mutation.operation.as_str()));
            let lined = state.lined.iter().any(|lined| lined.operation.as_deref() == Some(mutation.operation.as_str()));
            // A device from before landings came pushed asks again for words
            // still in line: it is answered as the first time, once they land.
            if (lined || pending.is_some()) && !bounded && receipt.landed.is_none() {
                drop(cell);
                drop(_reading);
                return legacy_lined::answer(node, id, &mutation.operation).await.map(|answer| (answer, false));
            }
            let answer = answer::Answer::state(
                state,
                bounded,
                receipt.landed.clone(),
                receipt.delivery.clone().or_else(|| pending.map(|flight| flight.delivery.id.clone())),
            );
            let accepted = bounded && receipt.delivery.is_none() && (pending.is_some() || lined);
            // Words left queued, asked for again: let go of only once on
            // the disk, as the first time (`settling`) — their commit may
            // have been the last process's, not yet synchronized.
            let queued = sends(&mutation.change) && !accepted && receipt.landed.is_none() && receipt.delivery.is_none();
            if queued {
                drop(cell);
                drop(_reading);
                settling::durable(node, Some((id, mutation.operation.as_str()))).await?;
            }
            return Ok((answer, accepted));
        }
        if let Some(outcome) = receipts::outcome(node, id, state, &mutation.operation).await? {
            if let Some(error) = outcome.failure {
                return Err(Failure::new(error));
            }
            return Ok((answer::Answer::state(state, bounded, outcome.landed, outcome.delivery), false));
        }
        if let Some(flight) = state
            .sending
            .iter()
            .chain(&state.pending)
            .find(|flight| flight.operation.as_deref() == Some(mutation.operation.as_str()))
        {
            return Ok((answer::Answer::state(state, bounded, None, Some(flight.delivery.id.clone())), true));
        }
        if matches!(&mutation.change, Change::SendDocument { epoch: None, .. }) {
            return Err(failure("the captured send requires its draft epoch; synchronize before sending"));
        }
        // An edit of an epoch of the draft sent since, from a device whose
        // answer and receipt are gone, was done if that epoch took it: it is
        // not done again in the new one (review 2.1).
        if let Change::Edit { target: fastctx_types::composer::Target::Draft, epoch, .. } = &mutation.change
            && *epoch < state.draft.epoch
            && receipts::taken(node, id, state, *epoch, &mutation.operation).await?
        {
            return Ok((answer::Answer::state(state, bounded, None, None), false));
        }
        if let Change::Edit { target, epoch, .. } = &mutation.change {
            cell.as_mut().expect("loaded").document(target, *epoch)?.reconcile_legacy();
        }
        let state = cell.as_ref().expect("loaded");
        limits::edit(state, &mutation)?;
        if let Change::Attach { attachment, .. } = &mutation.change {
            crate::attachments::resolve(node, id, std::slice::from_ref(attachment)).await?;
        }
        let was_uploaded = uploads::listed(state);
        let mut next = cell.take().expect("loaded");
        let mut flight = None;
        let mut entry = None;
        // Lined up to go once what is before them has: the node has them,
        // and says how they went when they have.
        let mut lined = false;
        match &mutation.change {
            Change::Send { .. } | Change::SendDocument { .. } => {
                let captured =
                    sending::capture(node, id, &mut next, from, &mutation.change, &mutation.operation, captured)?;
                (entry, flight, lined) = (Some(captured.entry), captured.flight, captured.lined);
            }
            // Asked while other words are being handed over, or while the
            // turn the user stopped has not ended, words sent at once go
            // right after them, never refused for it.
            Change::SendQueued { id: queued } => {
                let now = node.residents.now(id).ok_or_else(|| failure("unknown resident"))?;
                if next.sending.is_some() || !next.lined.is_empty() || submissions::stopping(node, id, &now) {
                    next.line_up(queued, Some(mutation.operation.clone()), false)?;
                    lined = true;
                } else {
                    flight = Some(next.reserve(queued, false, false)?);
                }
            }
            Change::Withdraw { send } => withdrawing::withdraw(node, id, &mut next, send).await?,
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
            Receipt {
                from: from.clone(),
                mutation: mutation.clone(),
                landed: None,
                delivery: None,
                failure: None,
                entry,
            },
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
        // Words sent on now, or lined up behind those going, are taken
        // without waiting for the disk (受理, design §3, §28.46): the sender
        // keeps them until it hears how they landed, and what makes them
        // safe is their record in the asset library, waited for before they
        // are handed on (已落盘). Words left in the queue of a busy
        // conversation are let go by the sender as it hears them taken: the
        // queue is their keeper. From the native input box they are taken
        // without waiting too — it sees them queued at once — and answered
        // once on the disk (`settling`); an older caller's answer waits for
        // the disk.
        let sent = sends(&mutation.change);
        let queued = sent && answer.is_none() && !lined;
        let relaxed = sent && (!queued || bounded);
        // Shown in no state until a barrier has it (`settling`).
        let withheld = queued && bounded;
        let first = withheld && settling::withhold(node, id, &mutation.operation);
        let published = storage::publish_at(node, id, &mut cell, next, !relaxed, move || {
            if let Some(callback) = callback {
                callback();
            }
            fastctx_platform::task::spawn_on(&runtime, async move {
                crate::attachments::forsake(&committed_node, &committed_id, &forsaken);
            });
        })
        .await;
        if let Err(failure) = published {
            // Failed for certain, there is no send to withhold; a commit whose
            // outcome is not known may stand, and stays withheld until a
            // barrier has it, asked again under its operation.
            // One withheld before this attempt — its outcome not known — stays
            // so: that attempt may stand though this one failed. The composer
            // changed under this one is answered so as well (the change may
            // be an earlier attempt of it, late): it is asked again, and kept
            // withheld until then — a send that never committed is in no
            // state to filter.
            let unknown = failure.code == Some(fastctx_types::ipc::FailureCode::Unanswered);
            if first && !unknown {
                settling::release(node, id, &mutation.operation);
            }
            return Err(failure);
        }
        let mut result = answer::Answer::state(cell.as_ref().expect("loaded"), bounded, None, None);
        drop(cell);
        drop(_reading);
        let Some((delivery, answer)) = answer else {
            schedule(node, id);
            if lined && !bounded {
                return legacy_lined::answer(node, id, &mutation.operation).await.map(|answer| (answer, false));
            }
            if withheld {
                settling::durable(node, Some((id, mutation.operation.as_str()))).await?;
            }
            return Ok((result, lined));
        };
        // The capture is committed; what it is handed on after is its record
        // in the asset library, on the disk (已落盘). Native clients receive
        // its operation state and never wait on a host for more than this
        // foreground interval.
        let delivered = if bounded {
            match tokio::time::timeout(std::time::Duration::from_millis(100), answer).await {
                Ok(Ok(answer)) => answer,
                Ok(Err(_)) | Err(_) => {
                    result.delivery = Some(delivery);
                    return Ok((result, true));
                }
            }
        } else {
            match answer.await {
                Ok(answer) => answer,
                // The dispatch ended without answering. The capture is
                // committed all the same: the operation's state tells how
                // it ended, not an error here.
                Err(_) => {
                    result.delivery = Some(delivery);
                    return Ok((result, true));
                }
            }
        };
        let cell = node.composers.cell(id);
        let mut cell = cell.lock().await;
        load(node, id, &mut cell).await?;
        delivered.map(|landed| {
            (answer::Answer::state(cell.as_ref().expect("loaded"), bounded, landed, Some(delivery)), false)
        })
    })
}

pub fn changed(node: &Arc<Node>, id: &ResidentId) {
    schedule(node, id);
}

/// Whether `change` sends words: from the input, captured, or from the queue.
fn sends(change: &Change) -> bool {
    matches!(change, Change::Send { .. } | Change::SendDocument { .. } | Change::SendQueued { .. })
}
