//! What members ask of a move: the super administrator, from a device
//! logged in to its account, and the machine the hub moves to.

use std::net::SocketAddr;
use std::sync::Arc;

use fastctx_net::moving::{ArrivalReport, ArrivalRequest, Identity};
use fastctx_net::sign::{Bytes, Secret};
use fastctx_net::wire::Told;
use fastctx_types::hub_move::{HubMove, HubMoveReply, HubMoveRequest, MoveStage, MoveTarget, OldPlace, Waiting};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;

use super::Outgoing;
use crate::Hub;
use crate::store::Store;

/// Answers the super administrator: `account` must be the hub's super
/// administrator, logged in on `asker`.
pub(crate) fn ask(
    hub: &Arc<Hub>,
    asker: &MachineName,
    account: &AccountName,
    request: HubMoveRequest,
) -> Result<Told, String> {
    {
        let store = hub.store();
        let directory = &store.book.directory;
        if !directory.logged_in(asker, account) {
            return Err(format!("{asker} is not logged in to the account {account}"));
        }
        if !directory.is_super(account) {
            return Err(format!("only the hub's super administrator moves the hub, and {account} is not it"));
        }
    }
    let reply = match request {
        HubMoveRequest::Targets => targets(hub, account),
        HubMoveRequest::Status => Ok(moves(hub)),
        HubMoveRequest::Start { to, addresses } => start(hub, account, to, addresses),
        HubMoveRequest::Retry => retry(hub),
        HubMoveRequest::Cancel => cancel(hub),
        HubMoveRequest::StopForwarding { id } => super::follow::stop_early(hub, &id).map(|()| moves(hub)),
        HubMoveRequest::DeleteOld { id } => super::follow::delete_old(hub, &id).map(|()| moves(hub)),
    }?;
    Ok(Told::HubMove { reply })
}

/// The account's own machines as places for the hub. The hub sees where
/// each connection comes from; the machine itself is asked for the rest.
fn targets(hub: &Hub, account: &AccountName) -> Result<HubMoveReply, String> {
    let store = hub.store();
    let sessions = hub.sessions();
    let pending = store.outgoing.as_ref().filter(|outgoing| outgoing.pending());
    let host = store.moves.last().map(|incoming| incoming.to.clone());
    let machines = store
        .book
        .directory
        .machines()
        .filter(|member| member.home.as_ref() == Some(account))
        .map(|member| {
            let session = sessions.get(&member.name);
            let seen = session.filter(|session| session.through.is_none()).map(|session| session.peer.ip());
            // Where a move brought the hub is sure; a connection from the
            // hub's own loopback likely comes from where it runs, but a
            // forwarder on that machine looks the same, so only the first
            // makes a machine unavailable.
            let moved_here = host.as_ref() == Some(&member.name);
            let hosts_hub = moved_here || seen.is_some_and(|ip| ip.is_loopback());
            let unavailable = if moved_here {
                Some("the hub runs there now".to_string())
            } else if store.book.directory.shares.iter().any(|share| share.machine == member.name) {
                Some(fastctx_types::sharing::RECALL_BEFORE_MOVE.into())
            } else if session.is_none() {
                Some("it is offline".to_string())
            } else {
                pending.map(|outgoing| format!("the hub is moving to {} now", outgoing.to))
            };
            MoveTarget {
                name: member.name.clone(),
                online: session.is_some(),
                os: None,
                form: None,
                sleeps: false,
                interfaces: Vec::new(),
                seen,
                hosts_hub,
                unavailable,
            }
        })
        .collect();
    Ok(HubMoveReply::Targets { machines, port: Some(store.hub.listen.port()) })
}

/// The move under way or last called off, the moves called off whose new
/// place has not given the hub up yet, then the moves that brought the hub
/// here, newest first.
pub(crate) fn moves(hub: &Hub) -> HubMoveReply {
    let store = hub.store();
    HubMoveReply::Moves { moves: listed(hub, &store) }
}

pub(crate) fn listed(hub: &Hub, store: &Store) -> Vec<HubMove> {
    let mut moves: Vec<HubMove> = store.outgoing.iter().map(Outgoing::view).collect();
    let current = store.outgoing.as_ref().map(|outgoing| outgoing.id.as_str());
    moves
        .extend(store.called_off.iter().rev().filter(|untold| Some(untold.id.as_str()) != current).map(Outgoing::view));
    moves.extend(
        store
            .moves
            .iter()
            .rev()
            .map(|incoming| incoming.view(&store.book.directory, hub.moving.old_connected(&incoming.id))),
    );
    moves
}

fn start(
    hub: &Arc<Hub>,
    account: &AccountName,
    to: MachineName,
    addresses: Vec<SocketAddr>,
) -> Result<HubMoveReply, String> {
    if addresses.is_empty() {
        return Err("give the address the hub is to be reached at there".to_string());
    }
    {
        let mut store = hub.store();
        if let Some(outgoing) = store.outgoing.as_ref().filter(|outgoing| outgoing.pending()) {
            return Err(format!("the hub is moving to {} already", outgoing.to));
        }
        let member = store.book.directory.member(&to).ok_or_else(|| format!("there is no machine named {to}"))?;
        if member.home.as_ref() != Some(account) {
            return Err(format!(
                "{to} is not one of {account}'s own machines: whoever holds the hub's machine holds the network, so it moves only to the super administrator's machines"
            ));
        }
        if store.book.directory.shares.iter().any(|share| store.book.directory.same(&share.machine, &to)) {
            return Err(fastctx_types::sharing::RECALL_BEFORE_MOVE.into());
        }
        if !hub.sessions().contains_key(&to) {
            return Err(format!("{to} is offline"));
        }
        // A machine connecting from the hub's own loopback is likely where
        // it runs, but only the targets say so: what is sure is where a move
        // brought the hub, and the new place refuses a directory that holds
        // a hub.
        if store.moves.last().is_some_and(|incoming| incoming.to == to) {
            return Err(format!("the hub runs on {to} already"));
        }
        if store.called_off.iter().any(|untold| untold.to == to) {
            return Err(format!(
                "a move to {to} was called off and {to} has not given the hub up yet; it does once the hub reaches it, or once {to} is online"
            ));
        }
        if addresses.iter().all(|address| store.hub.public.contains(address)) {
            return Err("the hub is reached at these addresses already".to_string());
        }
        let outgoing = Outgoing {
            id: super::new_id().map_err(|error| format!("making the move's id: {error}"))?,
            to,
            addresses,
            from: store.hub.public.clone(),
            by: account.clone(),
            started: Timestamp::now(),
            key: Secret::generate().map_err(|error| format!("making the move's key: {error}"))?,
            stage: MoveStage::Preparing,
            error: None,
            waiting: None,
            prepared: false,
            installed: false,
            switched: None,
            old: None,
            remaining: None,
        };
        hub.log.line(format_args!(
            "{account} moves the hub to {} at {} (move {})",
            outgoing.to,
            list(&outgoing.addresses),
            outgoing.id
        ));
        store.outgoing = Some(outgoing);
        store.save_outgoing().map_err(|error| format!("recording the move: {error}"))?;
    }
    hub.changed();
    super::out::drive(hub);
    Ok(moves(hub))
}

fn retry(hub: &Arc<Hub>) -> Result<HubMoveReply, String> {
    {
        let mut store = hub.store();
        let outgoing = store.outgoing.as_mut().filter(|outgoing| outgoing.pending()).ok_or("no move is under way")?;
        outgoing.error = None;
        store.save_outgoing().map_err(|error| format!("recording the move: {error}"))?;
    }
    hub.changed();
    super::out::drive(hub);
    Ok(moves(hub))
}

fn cancel(hub: &Arc<Hub>) -> Result<HubMoveReply, String> {
    {
        let mut store = hub.store();
        let outgoing = store.outgoing.as_mut().filter(|outgoing| outgoing.pending()).ok_or("no move is under way")?;
        if outgoing.stage >= MoveStage::Switching || hub.moving.frozen() {
            return Err("the hub is handing over to the new place now; the move can no longer be called off".into());
        }
        outgoing.stage = MoveStage::Cancelled;
        outgoing.waiting = None;
        outgoing.error = None;
        hub.log.line(format_args!("the move {} to {} is called off", outgoing.id, outgoing.to));
        // The new place made the hub's directory: it gives it up when the hub
        // reaches it, or deletes it itself when no service runs it.
        let untold = outgoing.prepared.then(|| outgoing.clone());
        store.save_outgoing().map_err(|error| format!("recording the move: {error}"))?;
        if let Some(untold) = untold {
            store.called_off.push(untold);
            store.save_called_off().map_err(|error| format!("recording the move: {error}"))?;
        }
    }
    hub.changed();
    super::out::drive(hub);
    Ok(moves(hub))
}

/// Answers the machine the hub moves to.
pub(crate) fn arrival(hub: &Arc<Hub>, asker: &MachineName, request: ArrivalRequest) -> Result<Told, String> {
    let id = match &request {
        ArrivalRequest::Take { id } | ArrivalRequest::Report { id, .. } => id.clone(),
    };
    let mut store = hub.store();
    if let ArrivalRequest::Report { report: ArrivalReport::Gone, .. } = &request {
        let before = store.called_off.len();
        store.called_off.retain(|untold| !(untold.id == id && untold.to == *asker));
        if store.called_off.len() != before {
            hub.log.line(format_args!("{asker} holds nothing of the move {id}, called off"));
            store.save_called_off().map_err(|error| format!("recording the move: {error}"))?;
            drop(store);
            hub.changed();
        }
        return Ok(Told::Done);
    }
    let outgoing = store
        .outgoing
        .as_ref()
        .filter(|outgoing| outgoing.pending() && outgoing.id == id && outgoing.to == *asker)
        .ok_or_else(|| format!("the hub is not moving to {asker} in the move {id}"))?
        .clone();
    match request {
        ArrivalRequest::Take { .. } => {
            let dir = store.dir().to_path_buf();
            let read = |path: std::path::PathBuf| {
                std::fs::read(&path).map(Bytes).map_err(|error| format!("reading {}: {error}", path.display()))
            };
            let identity = Identity {
                id: store.hub.id.clone(),
                name: store.hub.name.clone(),
                self_registration: store.hub.self_registration,
                certificate: read(Store::certificate_file(&dir))?,
                key: read(Store::key_file(&dir))?,
                secrets: read(Store::secrets_file(&dir))?,
                mover: outgoing.key().public(),
                addresses: outgoing.addresses.clone(),
                from: outgoing.from.clone(),
            };
            hub.log.line(format_args!("{asker} took the hub's identity for the move {id}"));
            Ok(Told::HubIdentity { identity: Box::new(identity) })
        }
        ArrivalRequest::Report { report, .. } => {
            let record = store.outgoing.as_mut().expect("found above");
            match report {
                ArrivalReport::Prepared { dir, os } => {
                    hub.log.line(format_args!("{asker} made the new hub's directory {dir}"));
                    record.stage = record.stage.max(MoveStage::Service);
                    record.prepared = true;
                    if !record.installed {
                        record.waiting = Some(Waiting::Service { dir, os });
                    }
                    record.error = None;
                }
                ArrivalReport::Installed => {
                    hub.log.line(format_args!("{asker} installed the new hub's service"));
                    record.installed = true;
                    record.stage = record.stage.max(MoveStage::Reaching);
                    record.waiting = None;
                    record.error = None;
                }
                ArrivalReport::Failed { reason } => {
                    hub.log.line(format_args!("{asker} could not take the hub: {reason}"));
                    record.error = Some(reason);
                }
                ArrivalReport::Gone => unreachable!("answered above"),
            }
            store.save_outgoing().map_err(|error| format!("recording the move: {error}"))?;
            drop(store);
            hub.changed();
            super::out::drive(hub);
            Ok(Told::Done)
        }
    }
}

/// The moves `me` is told of in its view: every one when it is a device of
/// the super administrator, which sees the whole directory, and the move
/// to it when it is where the hub moves.
pub(crate) fn told(hub: &Hub, store: &Store, me: &MachineName, whole: bool) -> Vec<HubMove> {
    if whole {
        return listed(hub, store);
    }
    let current = store.outgoing.as_ref().map(|outgoing| outgoing.id.as_str());
    store
        .outgoing
        .iter()
        .chain(store.called_off.iter().filter(|untold| Some(untold.id.as_str()) != current))
        .filter(|moving| moving.to == *me)
        .map(Outgoing::view)
        .collect()
}

/// What the old place of `incoming` does now, as the new hub has it.
pub(crate) fn old_of(store: &Store, id: &str) -> Option<OldPlace> {
    store.moves.iter().find(|incoming| incoming.id == id).map(|incoming| incoming.old.clone())
}

fn list(addresses: &[SocketAddr]) -> String {
    addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}
