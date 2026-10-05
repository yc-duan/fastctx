//! A hidden Android page releases its live stream. History replaces the
//! page's previous reading when it returns, avoiding an unbounded JS backlog.

use fastctx_client::{Conversation, World};
use fastctx_net::link::Status;
use fastctx_types::session::SessionEvent;
use fastctx_types::{MachineName, ResidentId};
use tauri::ipc::Channel;

use crate::Piece;

pub(crate) async fn open(
    world: &World,
    resident: ResidentId,
    machine: Option<MachineName>,
) -> Result<(Conversation, Vec<SessionEvent>), String> {
    let mut connection =
        world.device().expect("Android device").connection_changes().map_err(|error| error.to_string())?;
    loop {
        let state = connection.borrow_and_update().clone();
        match state {
            Status::Stopped { reason } => return Err(reason),
            Status::Connecting { .. } => connection.changed().await.map_err(|error| error.to_string())?,
            Status::Online { .. } => {
                let read = tokio::select! {
                    changed = connection.changed() => { changed.map_err(|error| error.to_string())?; continue; }
                    read = Conversation::watch(world, resident.clone(), machine.clone()) => read,
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

pub(crate) async fn stream(
    world: World,
    resident: ResidentId,
    machine: Option<MachineName>,
    initial: Conversation,
    pieces: Channel<Piece>,
) {
    let mut connection = match world.device().expect("Android device").connection_changes() {
        Ok(connection) => connection,
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
                read = open(&world, resident.clone(), machine.clone()) => read,
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
                    Some(conversation) => conversation.next().await,
                    None => std::future::pending().await,
                }
            } => {
                match piece {
                    Ok(Some(moment)) => if pieces.send(Piece::Event { event: moment.event }).is_err() { return; },
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
