//! A hidden Android page releases its live stream. History replaces the
//! page's previous reading when it returns, avoiding an unbounded JS backlog.
//! What this phone keeps of a conversation is shown at once, connected or
//! not; only what it lacks is read when it is connected again. A reading
//! that fails for a passing reason is opened again after a wait, paced as
//! every stream is (`fastctx_client::Again`); only a deleted conversation,
//! or one this account may not read, ends the watch.

use fastctx_client::{Again, Conversation, Update, World};
use fastctx_net::link::Status;
use fastctx_types::access::Written;
use fastctx_types::session::SessionEvent;
use fastctx_types::{Address, ResidentId};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use crate::watch_delivery::Sender;

use crate::transcript::Transcript;
use crate::{NotWatched, Piece};

/// The history a reading gave, as what the page's watch is given next.
fn given(transcript: &Mutex<Transcript>, history: &[SessionEvent]) -> Option<Piece> {
    transcript
        .lock()
        .expect("transcript")
        .history(history)
        .unwrap_or_else(|error| Some(Piece::Ended { error: Some(error) }))
}

/// What the page is given for `update`, through what it holds.
fn handed(transcript: &Mutex<Transcript>, update: Update) -> Option<Piece> {
    transcript
        .lock()
        .expect("transcript")
        .take(update)
        .unwrap_or_else(|error| Some(Piece::Ended { error: Some(error) }))
}

/// Tells the page its reading broke for `why`; false once the page let go.
fn interrupted(pieces: &Sender<Piece>, again: &mut Again, why: String) -> bool {
    pieces.send(Piece::interrupted(&again.broke(why))).is_ok()
}

/// The wait before the conversation is opened again: as long as `again`
/// says, or until the connection or the page changes; false once either is
/// gone.
async fn waited(
    world: &World,
    machine: Option<&Written>,
    again: &Again,
    connection: &mut watch::Receiver<Status>,
    visible: &mut watch::Receiver<bool>,
) -> bool {
    tokio::select! {
        changed = connection.changed() => changed.is_ok(),
        changed = visible.changed() => changed.is_ok(),
        () = again.pause(world, machine) => true,
    }
}

/// Reads the rest of what this phone keeps of the conversation; false once
/// the watch is over. A reading that failed for a passing reason lets the
/// conversation go, to be opened again.
async fn rest(
    conversation: &mut Option<Conversation>,
    transcript: &Mutex<Transcript>,
    pieces: &Sender<Piece>,
    again: &mut Again,
) -> bool {
    let Some(kept) = conversation.as_mut() else { return true };
    match kept.update().await {
        Ok(update) => deliver(pieces, handed(transcript, update)),
        Err(error) if error.is_deleted() || error.is_refused() => {
            let _ = pieces.send(Piece::failed(&error));
            false
        }
        Err(error) => {
            *conversation = None;
            interrupted(pieces, again, error.to_string())
        }
    }
}

/// Sends the page what it is given next; false once the watch is over, as
/// the page let go or what it holds could not be kept.
fn deliver(pieces: &Sender<Piece>, piece: Option<Piece>) -> bool {
    let Some(piece) = piece else { return true };
    let ended = piece.last();
    pieces.send(piece).is_ok() && !ended
}

pub(crate) async fn open(
    world: &World,
    resident: ResidentId,
    machine: Option<Written>,
    again: bool,
) -> Result<(Conversation, Vec<SessionEvent>), NotWatched> {
    if let Some(machine) = &machine
        && let Some(cache) = world.cache().cloned()
    {
        let address = Address { machine: machine.name.clone(), resident: resident.clone() };
        let held = tokio::task::spawn_blocking(move || cache.holds(&address))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        if held {
            return Ok(opening(world, resident, machine.clone().into(), again).await?);
        }
    }
    let (_, mut connection) =
        world.device().expect("Android device").machine_changes(machine.as_ref()).map_err(|error| error.to_string())?;
    loop {
        let state = connection.borrow_and_update().clone();
        match state {
            Status::Stopped { reason } => return Err(reason.into()),
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
                return Ok(read?);
            }
        }
    }
}

/// Opens the conversation through what this phone keeps, where its
/// machine is known; `again` when the page already shows it.
async fn opening(
    world: &World,
    resident: ResidentId,
    machine: Option<Written>,
    again: bool,
) -> Result<(Conversation, Vec<SessionEvent>), fastctx_client::Error> {
    match machine {
        Some(machine) if again => Conversation::reopen(world, resident, machine).await,
        Some(machine) => Conversation::open(world, resident, machine).await,
        None => Conversation::watch(world, resident, None).await,
    }
}

/// A page that cannot be live now, hidden or with its connection down.
fn paused() -> Piece {
    Piece::Interrupted { error: None, since: crate::epoch_ms(std::time::SystemTime::now()) }
}

pub(crate) async fn stream(
    world: World,
    resident: ResidentId,
    machine: Option<Written>,
    initial: Conversation,
    transcript: Arc<Mutex<Transcript>>,
    pieces: Sender<Piece>,
) {
    let mut again = Again::default();
    let mut conversation = Some(initial);
    if conversation.as_ref().is_some_and(Conversation::has_cached_rest)
        && !rest(&mut conversation, &transcript, &pieces, &mut again).await
    {
        return;
    }
    let mut connection = match world.device().expect("Android device").machine_changes(machine.as_ref()) {
        Ok((_, connection)) => connection,
        Err(error) => {
            let _ = pieces.send(Piece::Ended { error: Some(error.to_string()) });
            return;
        }
    };
    let mut visible = super::visibility();
    loop {
        let online = matches!(*connection.borrow_and_update(), Status::Online { .. });
        let shown = *visible.borrow_and_update();
        if matches!(*connection.borrow(), Status::Stopped { .. }) {
            let _ = pieces.send(Piece::Ended { error: Some("this device's connection has stopped".into()) });
            return;
        }
        if shown
            && !online
            && conversation.as_ref().is_some_and(Conversation::has_cached_rest)
            && !rest(&mut conversation, &transcript, &pieces, &mut again).await
        {
            return;
        }
        if !online || !shown {
            if conversation.take().is_some() && pieces.send(paused()).is_err() {
                return;
            }
        } else if conversation.is_none() {
            let read = tokio::select! {
                changed = connection.changed() => { if changed.is_err() { return } continue; }
                changed = visible.changed() => { if changed.is_err() { return } continue; }
                read = open(&world, resident.clone(), machine.clone(), true) => read,
            };
            match read {
                Ok((fresh, history)) => {
                    again.came();
                    if !deliver(&pieces, given(&transcript, &history)) {
                        return;
                    }
                    conversation = Some(fresh);
                }
                Err(error) if error.deleted || error.refused => {
                    let _ = pieces.send(error.piece());
                    return;
                }
                Err(error) => {
                    if !interrupted(&pieces, &mut again, error.message)
                        || !waited(&world, machine.as_ref(), &again, &mut connection, &mut visible).await
                    {
                        return;
                    }
                    continue;
                }
            }
        }
        tokio::select! {
            changed = connection.changed() => {
                if changed.is_err() { return; }
                if conversation.take().is_some() && pieces.send(paused()).is_err() { return; }
            }
            changed = visible.changed() => { if changed.is_err() { return; } }
            piece = async {
                match conversation.as_mut() {
                    Some(conversation) => conversation.update().await,
                    None => std::future::pending().await,
                }
            } => {
                match piece {
                    // It opens itself again, as `fastctx_client::Conversation`
                    // paces it; what it hands over goes through what the page
                    // holds (`crate::transcript`).
                    Ok(update) => {
                        if !deliver(&pieces, handed(&transcript, update)) { return; }
                    }
                    Err(error) if error.is_deleted() || error.is_refused() => {
                        let _ = pieces.send(Piece::failed(&error));
                        return;
                    }
                    // Opened again after the wait, or sooner when the
                    // connection or the page changes.
                    Err(error) => {
                        conversation = None;
                        if !interrupted(&pieces, &mut again, error.to_string()) { return; }
                        if !connection.has_changed().unwrap_or(false)
                            && !waited(&world, machine.as_ref(), &again, &mut connection, &mut visible).await
                        {
                            return;
                        }
                    }
                }
            }
        }
    }
}
