//! The new hub after a switch: which machines still come through the old
//! place, and the old place itself, connected to it.
//!
//! A machine follows once it connects here other than through the old
//! place: it dialed an address that is not one of the old place's. Every
//! machine hears where the hub is now from its view; one that cannot reach
//! the new address yet keeps reaching it through the old place, and is
//! counted as not followed, since stopping the old place would cut it off.
//! When every member there was at the switch has followed or is gone, the
//! old place is told to stop; the super administrator may stop it before.

use std::net::SocketAddr;
use std::sync::Arc;

use fastctx_net::moving::HandoverRole;
use fastctx_net::sign::{Domain, Signature};
use fastctx_net::wire::{HEARTBEAT, Refusal, SILENCE};
use fastctx_types::MachineName;
use fastctx_types::hub_move::OldPlace;
use futures_util::{SinkExt as _, StreamExt as _};
use jiff::Timestamp;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::frames::{self, ToNew, ToOld};
use crate::{Hub, Socket, refuse};

/// Notes that `name` connected, dialing `dialed`.
pub(crate) fn came(hub: &Hub, name: &MachineName, dialed: Option<SocketAddr>) {
    let Some(dialed) = dialed else { return };
    let mut followed = false;
    {
        let mut store = hub.store();
        for incoming in store.moves.iter_mut().filter(|incoming| incoming.forwarding()) {
            if !incoming.from.contains(&dialed)
                && incoming.members.contains_key(name)
                && incoming.followed.insert(name.clone())
            {
                hub.log.line(format_args!("{name} followed the hub here (move {})", incoming.id));
                followed = true;
            }
        }
        if followed && let Err(error) = store.save_moves() {
            hub.log.line(format_args!("recording that {name} followed: {error}"));
        }
    }
    if followed {
        settle(hub);
    }
}

/// Tells each old place still forwarding how many have not followed, and
/// stops it when none is left.
pub(crate) fn settle(hub: &Hub) {
    // Nothing changes while the hub hands its records over.
    if hub.moving.frozen() {
        return;
    }
    let mut told = Vec::new();
    let mut stopped = false;
    {
        let mut store = hub.store();
        let store = &mut *store;
        for incoming in store.moves.iter_mut().filter(|incoming| incoming.forwarding()) {
            let (machines, devices) = incoming.remaining(&store.book.directory);
            if machines.is_empty() && devices.is_empty() {
                hub.log.line(format_args!(
                    "every machine followed the hub here; the old place stops (move {})",
                    incoming.id
                ));
                incoming.old = OldPlace::Stopped { at: Timestamp::now(), early: false };
                told.push((incoming.id.clone(), ToOld::Stop { early: false }));
                stopped = true;
            } else {
                told.push((incoming.id.clone(), ToOld::Following { machines: machines.len(), devices: devices.len() }));
            }
        }
        if stopped && let Err(error) = store.save_moves() {
            hub.log.line(format_args!("recording that the old place stops: {error}"));
        }
    }
    for (id, frame) in told {
        tell(hub, &id, frame);
    }
    if stopped {
        // Members no longer need the old place's addresses.
        hub.changed();
    }
}

fn tell(hub: &Hub, id: &str, frame: ToOld) {
    let out = hub.moving.olds.lock().expect("never poisoned").get(id).cloned();
    if let Some(out) = out {
        let _ = out.send(frame);
    }
}

/// The super administrator stops the old place of the move `id` now.
pub(crate) fn stop_early(hub: &Hub, id: &str) -> Result<(), String> {
    {
        let mut store = hub.store();
        let incoming = store.moves.iter_mut().find(|incoming| incoming.id == id).ok_or_else(|| unknown(id))?;
        if !incoming.forwarding() {
            return Err("the old place does not forward any more".to_string());
        }
        incoming.old = OldPlace::Stopped { at: Timestamp::now(), early: true };
        hub.log.line(format_args!("the old place of the move {id} is stopped before every machine followed"));
        store.save_moves().map_err(|error| format!("recording the stop: {error}"))?;
    }
    tell(hub, id, ToOld::Stop { early: true });
    hub.changed();
    Ok(())
}

/// The super administrator deletes the old place's data, once it stopped
/// forwarding; it does when it is connected, now or later.
pub(crate) fn delete_old(hub: &Hub, id: &str) -> Result<(), String> {
    {
        let mut store = hub.store();
        let incoming = store.moves.iter_mut().find(|incoming| incoming.id == id).ok_or_else(|| unknown(id))?;
        match incoming.old {
            OldPlace::Forwarding => return Err("the old place still forwards; stop it first".to_string()),
            OldPlace::Deleted { .. } => return Err("the old place's data is deleted already".to_string()),
            OldPlace::Stopped { .. } => incoming.old = OldPlace::Deleting { at: Timestamp::now() },
            OldPlace::Deleting { .. } => {}
        }
        hub.log.line(format_args!("the old place of the move {id} is to delete its data"));
        store.save_moves().map_err(|error| format!("recording the deletion: {error}"))?;
    }
    tell(hub, id, ToOld::Delete);
    hub.changed();
    Ok(())
}

fn unknown(id: &str) -> String {
    format!("the hub came here by no move {id}")
}

/// The old place of the move `id`, connected.
pub(crate) async fn old_connected(
    hub: Arc<Hub>,
    mut socket: Socket,
    binding: [u8; 32],
    id: String,
    role: HandoverRole,
    proof: Signature,
) {
    let mover = hub.store().moves.iter().find(|incoming| incoming.id == id).map(|incoming| incoming.mover);
    let Some(mover) = mover else {
        refuse(&mut socket, Refusal::Broken { reason: unknown(&id) }).await;
        return;
    };
    if role != HandoverRole::Old {
        refuse(&mut socket, Refusal::Broken { reason: "this hub took over already".to_string() }).await;
        return;
    }
    if mover.verify(Domain::Handover, &binding, &proof).is_err() {
        hub.log
            .line(format_args!("refused a connection for the move {id}: the proof of the move's key does not verify"));
        refuse(&mut socket, Refusal::Broken { reason: "the proof of the move's key does not verify".to_string() })
            .await;
        return;
    }
    let deleted = match frames::frame::<_, ToNew>(&mut socket).await {
        Ok(ToNew::Old { deleted, .. }) => deleted,
        Ok(other) => {
            hub.log.line(format_args!("the old place of the move {id} said {other:?} first"));
            return;
        }
        Err(error) => {
            hub.log.line(format_args!("the old place of the move {id}: {error}"));
            return;
        }
    };
    hub.log.line(format_args!("the old place of the move {id} is connected"));
    if deleted {
        noted_deleted(&hub, &id);
    }
    // What it is to do now, as this hub has it; what changes later comes
    // through the channel the hub keeps for it, which ends this connection
    // when another replaces it or the hub stops taking anything.
    let (out, mut outgoing) = mpsc::unbounded_channel();
    match super::requests::old_of(&hub.store(), &id) {
        Some(OldPlace::Stopped { early, .. }) => {
            let _ = out.send(ToOld::Stop { early });
        }
        Some(OldPlace::Deleting { .. }) => {
            let _ = out.send(ToOld::Stop { early: false });
            let _ = out.send(ToOld::Delete);
        }
        Some(OldPlace::Forwarding | OldPlace::Deleted { .. }) | None => {}
    }
    let mine = out.downgrade();
    hub.moving.olds.lock().expect("never poisoned").insert(id.clone(), out);
    settle(&hub);
    hub.changed();
    let (mut sink, mut stream) = socket.split();
    let mut heard = Instant::now();
    let mut beat = tokio::time::interval(HEARTBEAT);
    let ended = loop {
        tokio::select! {
            frame = outgoing.recv() => match frame {
                Some(frame) => if let Err(error) = frames::send(&mut sink, &frame).await { break error },
                None => break "another connection of the old place replaced this one".to_string(),
            },
            _ = beat.tick() => {
                if let Err(error) = frames::send(&mut sink, &ToOld::Working).await { break error }
            }
            () = tokio::time::sleep_until(heard + SILENCE) => {
                break format!("nothing was heard from it for {} s", SILENCE.as_secs());
            }
            message = stream.next() => {
                heard = Instant::now();
                match frames::parse::<ToNew>(message) {
                    Ok(Some(ToNew::Deleted)) => noted_deleted(&hub, &id),
                    Ok(Some(ToNew::Old { .. }) | None) => {}
                    Ok(Some(other)) => break format!("it said {other:?}"),
                    Err(error) => break error,
                }
            }
        }
    };
    {
        let mut olds = hub.moving.olds.lock().expect("never poisoned");
        if olds.get(&id).is_some_and(|current| mine.upgrade().is_some_and(|mine| mine.same_channel(current))) {
            olds.remove(&id);
        }
    }
    hub.log.line(format_args!("the old place of the move {id} disconnected: {ended}"));
    let _ = sink.close().await;
    hub.changed();
}

fn noted_deleted(hub: &Hub, id: &str) {
    {
        let mut store = hub.store();
        let Some(incoming) = store.moves.iter_mut().find(|incoming| incoming.id == id) else { return };
        if matches!(incoming.old, OldPlace::Deleted { .. }) {
            return;
        }
        incoming.old = OldPlace::Deleted { at: Timestamp::now() };
        hub.log.line(format_args!("the old place of the move {id} deleted its data"));
        if let Err(error) = store.save_moves() {
            hub.log.line(format_args!("recording that the old data is deleted: {error}"));
        }
    }
    hub.changed();
}
