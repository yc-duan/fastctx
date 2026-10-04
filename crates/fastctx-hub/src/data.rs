//! Data connections: one per transfer, paired by the hub and relayed byte
//! for byte, so a large transfer never holds up the heartbeats, calls and
//! letters on the control connections.
//!
//! Each end connects with the ticket its source signed. The first to arrive
//! waits for the other; the second hands its connection to the first, which
//! relays in both directions until either side ends. One message is in
//! flight per direction, so the slower side sets the pace.

use std::sync::Arc;

use fastctx_net::sign::{Domain, PublicKey, Signature, Signed};
use fastctx_net::wire::{Down, End, Refusal, Ticket};
use fastctx_types::MachineName;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::oneshot;

use crate::{Hub, Socket, refuse, send};

pub(crate) struct Waiting {
    ticket: Signed,
    end: End,
    /// The member the waiting end waits for.
    other: MachineName,
    hand_over: oneshot::Sender<Socket>,
}

pub(crate) async fn end(
    hub: Arc<Hub>,
    mut socket: Socket,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
    signed: Signed,
    end: End,
) {
    let (stream, me, other) = match check(&hub, &binding, &key, &proof, &signed, end) {
        Ok(checked) => checked,
        Err(reason) => {
            refuse(&mut socket, reason).await;
            return;
        }
    };
    let waiting = hub.waiting.lock().expect("never poisoned").remove(&stream);
    match waiting {
        Some(waiting) if waiting.ticket == signed && waiting.end != end => {
            let (source, sink) = if end == End::Source { (&me, &other) } else { (&other, &me) };
            hub.log.line(format_args!("a transfer from {source} to {sink} started"));
            // The first end relays; this one is handed over to it.
            if let Err(mut socket) = waiting.hand_over.send(socket) {
                refuse(&mut socket, Refusal::Broken { reason: "the other end left".to_string() }).await;
            }
        }
        Some(waiting) => {
            let reason = "two ends that do not match presented one stream".to_string();
            refuse(&mut socket, Refusal::Broken { reason: reason.clone() }).await;
            drop(waiting);
        }
        None => {
            let (hand_over, handed) = oneshot::channel();
            hub.waiting
                .lock()
                .expect("never poisoned")
                .insert(stream, Waiting { ticket: signed, end, other: other.clone(), hand_over });
            let peer = tokio::select! {
                handed = handed => handed.ok(),
                // The waiting end gave up or vanished.
                _ = socket.next() => None,
            };
            hub.waiting.lock().expect("never poisoned").remove(&stream);
            match peer {
                Some(peer) => {
                    let carried = relay(socket, peer).await;
                    let (source, sink) = if end == End::Source { (&me, &other) } else { (&other, &me) };
                    hub.log
                        .line(format_args!("a transfer from {source} to {sink} ended; {carried} bytes came from {me}"));
                }
                None => refuse(&mut socket, Refusal::Broken { reason: "the other end did not come".to_string() }).await,
            }
        }
    }
}

/// Checks the ticket and that the connecting machine is the end it says.
/// Returns the stream, the connecting member and the member at the other
/// end.
fn check(
    hub: &Hub,
    binding: &[u8; 32],
    key: &PublicKey,
    proof: &Signature,
    signed: &Signed,
    end: End,
) -> Result<(u64, MachineName, MachineName), Refusal> {
    let name = hub.member(key, proof, binding)?;
    let broken = |reason: String| Refusal::Broken { reason };
    let claimed: Ticket = signed.claim().map_err(|error| broken(format!("the ticket: {error}")))?;
    let (source_key, world) = {
        let store = hub.store();
        let source = store.table().get(&claimed.source).map(|member| member.key);
        (source, store.hub.world.clone())
    };
    let source_key = source_key.ok_or_else(|| broken(format!("{} is not a member", claimed.source)))?;
    let ticket: Ticket =
        signed.open(&source_key, Domain::Ticket).map_err(|error| broken(format!("the ticket: {error}")))?;
    if ticket.world != world || ticket.source == ticket.sink {
        return Err(broken("the ticket does not name two members of this World".to_string()));
    }
    let (me, other) = match end {
        End::Source => (&ticket.source, &ticket.sink),
        End::Sink => (&ticket.sink, &ticket.source),
    };
    if *me != name {
        return Err(broken(format!(
            "{name} is not the {} of this transfer",
            if end == End::Source { "source" } else { "sink" }
        )));
    }
    if !hub.sessions().contains_key(other) {
        return Err(broken(format!("{other} is not connected")));
    }
    Ok((ticket.stream, name, other.clone()))
}

/// Ends the waiting of everyone waiting for `name`, which left.
pub(crate) fn member_left(hub: &Hub, name: &MachineName) {
    hub.waiting.lock().expect("never poisoned").retain(|_, waiting| waiting.other != *name);
}

/// Relays between the two ends until either is done; returns how many bytes
/// passed from the first end to the second.
async fn relay(mut first: Socket, mut second: Socket) -> u64 {
    if send(&mut first, &Down::Paired).await.is_err() || send(&mut second, &Down::Paired).await.is_err() {
        return 0;
    }
    let mut carried = 0_u64;
    let (mut first_sink, mut first_stream) = first.split();
    let (mut second_sink, mut second_stream) = second.split();
    let forward = async {
        while let Some(Ok(message)) = first_stream.next().await {
            carried += message.len() as u64;
            if message.is_close() || second_sink.send(message).await.is_err() {
                break;
            }
        }
    };
    let backward = async {
        while let Some(Ok(message)) = second_stream.next().await {
            if message.is_close() || first_sink.send(message).await.is_err() {
                break;
            }
        }
    };
    // A WebSocket close does not pass through as half a close: when either
    // side is done, the transfer is.
    tokio::select! {
        () = forward => {}
        () = backward => {}
    }
    let _ = first_sink.close().await;
    let _ = second_sink.close().await;
    carried
}
