//! A hidden Android page releases its live stream. History replaces the
//! page's previous reading when it returns, avoiding an unbounded JS backlog.
//! What this phone keeps of a conversation is shown at once, connected or
//! not; only what it lacks is read when it is connected again.

use fastctx_client::{Conversation, Update, World};
use fastctx_net::link::Status;
use fastctx_types::session::SessionEvent;
use fastctx_types::{Address, MachineName, ResidentId};
use tauri::ipc::Channel;

use crate::Piece;

pub(crate) async fn open(
    world: &World,
    resident: ResidentId,
    machine: Option<MachineName>,
    again: bool,
) -> Result<(Conversation, Vec<SessionEvent>), String> {
    if let (Some(machine), Some(cache)) = (&machine, world.cache()) {
        let address = Address { machine: machine.clone(), resident: resident.clone() };
        if cache.holds(&address).map_err(|error| error.to_string())? {
            return opening(world, resident, Some(address.machine), again).await.map_err(|error| error.to_string());
        }
    }
    let (_, mut connection) =
        world.device().expect("Android device").machine_changes(machine.as_ref()).map_err(|error| error.to_string())?;
    loop {
        let state = connection.borrow_and_update().clone();
        match state {
            Status::Stopped { reason } => return Err(reason),
            Status::Connecting { .. } => connection.changed().await.map_err(|error| error.to_string())?,
            Status::Online { .. } => {
                let read = tokio::select! {
                    changed = connection.changed() => { changed.map_err(|error| error.to_string())?; continue; }
                    read = opening(world, resident.clone(), machine.clone(), again) => read,
                };
                // A foreground resume can replace the connection between
                // checking it and opening the read-only subscription.
                if read.is_err() && connection.has_changed().map_err(|error| error.to_string())? {
                    continue;
                }
                return read.map_err(|error| error.to_string());
            }
        }
    }
}

/// Opens the conversation through what this phone keeps, where its
/// machine is known; `again` when the page already shows it.
async fn opening(
    world: &World,
    resident: ResidentId,
    machine: Option<MachineName>,
    again: bool,
) -> Result<(Conversation, Vec<SessionEvent>), fastctx_client::Error> {
    match machine {
        Some(machine) if again => Conversation::reopen(world, Address { machine, resident }).await,
        Some(machine) => Conversation::open(world, Address { machine, resident }).await,
        None => Conversation::watch(world, resident, None).await,
    }
}

pub(crate) async fn stream(
    world: World,
    resident: ResidentId,
    machine: Option<MachineName>,
    initial: Conversation,
    pieces: Channel<Piece>,
) {
    let mut connection = match world.device().expect("Android device").machine_changes(machine.as_ref()) {
        Ok((_, connection)) => connection,
        Err(error) => {
            let _ = pieces.send(Piece::Ended { error: Some(error.to_string()) });
            return;
        }
    };
    let mut visible = super::visibility();
    let mut conversation = Some(initial);
    loop {
        let online = matches!(*connection.borrow_and_update(), Status::Online { .. });
        let shown = *visible.borrow_and_update();
        if matches!(*connection.borrow(), Status::Stopped { .. }) {
            let _ = pieces.send(Piece::Ended { error: Some("this device's connection has stopped".into()) });
            return;
        }
        if !online || !shown {
            conversation = None;
        } else if conversation.is_none() {
            let read = tokio::select! {
                changed = connection.changed() => { if changed.is_err() { return } continue; }
                changed = visible.changed() => { if changed.is_err() { return } continue; }
                read = open(&world, resident.clone(), machine.clone(), true) => read,
            };
            match read {
                Ok((fresh, history)) => {
                    if pieces.send(Piece::Reset { history }).is_err() {
                        return;
                    }
                    conversation = Some(fresh);
                }
                Err(error) => {
                    let _ = pieces.send(Piece::Ended { error: Some(error.to_string()) });
                    return;
                }
            }
        }
        tokio::select! {
            changed = connection.changed() => {
                if changed.is_err() { return; }
                conversation = None;
            }
            changed = visible.changed() => { if changed.is_err() { return; } }
            piece = async {
                match conversation.as_mut() {
                    Some(conversation) => conversation.update().await,
                    None => std::future::pending().await,
                }
            } => {
                match piece {
                    Ok(Some(Update::Moment(moment))) => if pieces.send(Piece::Event { event: moment.event }).is_err() { return; },
                    Ok(Some(Update::History(history))) => if pieces.send(Piece::Reset { history }).is_err() { return; },
                    ended => {
                        conversation = None;
                        let error = ended.err().map(|error| error.to_string());
                        if pieces.send(Piece::Interrupted { error }).is_err() { return; }
                        // A failed live stream waits for an actual connection
                        // or visibility change; it never replays a failed call.
                        if !connection.has_changed().unwrap_or(false) {
                            tokio::select! {
                                changed = connection.changed() => if changed.is_err() { return; },
                                changed = visible.changed() => if changed.is_err() { return; },
                            }
                        }
                    }
                }
            }
        }
    }
}
