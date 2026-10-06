//! Data connections at a relay. Each end comes from below, from the machine
//! itself or from a relay below that carries it. When the machine at the
//! other end is below this relay too, the two ends meet here, as at the hub;
//! otherwise this relay opens the end upward on behalf of the machine, and
//! passes the bytes between the two connections once the upstream pairs
//! it. Either way the ends meet at their nearest common upstream, and the
//! bytes, sealed for the two ends, are passed on unread.

use std::sync::Arc;

use fastctx_net::dial;
use fastctx_net::sign::{Domain, PublicKey, Signature, Signed};
use fastctx_net::wire::{self, Down, End, Intent, PROTOCOL, Refusal, Ticket, Up};
use fastctx_types::MachineName;
use futures_util::StreamExt as _;
use tokio::sync::oneshot;

use super::Shared;
use crate::{Socket, refuse, send};

pub(crate) struct Waiting {
    ticket: Signed,
    end: End,
    /// The member the waiting end waits for.
    pub(super) other: MachineName,
    hand_over: oneshot::Sender<Socket>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn end(
    shared: Arc<Shared>,
    mut socket: Socket,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
    signed: Signed,
    end: End,
    on_behalf: Option<MachineName>,
) {
    let (stream, me, other) = match check(&shared, &binding, &key, &proof, &signed, end, on_behalf).await {
        Ok(checked) => checked,
        Err(reason) => {
            refuse(&mut socket, reason).await;
            return;
        }
    };
    if shared.state().below.get(&other).is_some_and(|below| below.here()) {
        meet(&shared, socket, signed, end, stream, me, other).await;
    } else {
        carry(&shared, socket, signed, end, me).await;
    }
}

/// Checks the ticket, that the end comes from below this relay, and that
/// the machine it is for is the end it says. Returns the stream, the member
/// at this end and the member at the other.
async fn check(
    shared: &Shared,
    binding: &[u8; 32],
    key: &PublicKey,
    proof: &Signature,
    signed: &Signed,
    end: End,
    on_behalf: Option<MachineName>,
) -> Result<(u64, MachineName, MachineName), Refusal> {
    let connecting = shared.admit(key, proof, binding).await?;
    let broken = |reason: String| Refusal::Broken { reason };
    let table = shared.machine.table();
    let name = match on_behalf {
        Some(below) if table.carries(&connecting, &below) => below,
        Some(below) => return Err(broken(format!("{below}'s route does not lead through {connecting}"))),
        None => connecting,
    };
    let claimed: Ticket = signed.claim().map_err(|error| broken(format!("the ticket: {error}")))?;
    let source_key = table
        .get(&claimed.source)
        .map(|member| member.key)
        .ok_or_else(|| broken(format!("{} is not a member", claimed.source)))?;
    let ticket: Ticket =
        signed.open(&source_key, Domain::Ticket).map_err(|error| broken(format!("the ticket: {error}")))?;
    if ticket.world != shared.world || ticket.source == ticket.sink {
        return Err(broken("the ticket does not name two members of this World".to_string()));
    }
    let (this, other) = match end {
        End::Source => (&ticket.source, &ticket.sink),
        End::Sink => (&ticket.sink, &ticket.source),
    };
    if *this != name {
        return Err(broken(format!(
            "{name} is not the {} of this transfer",
            if end == End::Source { "source" } else { "sink" }
        )));
    }
    Ok((ticket.stream, name, other.clone()))
}

/// Both ends are below this relay: the first waits here for the second.
async fn meet(
    shared: &Shared,
    mut socket: Socket,
    signed: Signed,
    end: End,
    stream: u64,
    me: MachineName,
    other: MachineName,
) {
    // Taken out, or put in to wait, in one look: two ends that come at once
    // find each other.
    let (hand_over, handed) = oneshot::channel();
    let waiting = {
        let mut state = shared.state();
        let waiting = state.waiting.remove(&stream);
        if waiting.is_none() {
            state.waiting.insert(stream, Waiting { ticket: signed.clone(), end, other: other.clone(), hand_over });
        }
        waiting
    };
    match waiting {
        Some(waiting) if waiting.ticket == signed && waiting.end != end => {
            if let Err(mut socket) = waiting.hand_over.send(socket) {
                refuse(&mut socket, Refusal::Broken { reason: "the other end left".to_string() }).await;
            }
        }
        Some(_) => {
            let reason = "two ends that do not match presented one stream".to_string();
            refuse(&mut socket, Refusal::Broken { reason }).await;
        }
        None => {
            let peer = tokio::select! {
                handed = handed => handed.ok(),
                // The waiting end gave up or vanished.
                _ = socket.next() => None,
            };
            shared.state().waiting.remove(&stream);
            let Some(mut peer) = peer else {
                refuse(&mut socket, Refusal::Broken { reason: "the other end did not come".to_string() }).await;
                return;
            };
            if send(&mut socket, &Down::Paired).await.is_err() || send(&mut peer, &Down::Paired).await.is_err() {
                return;
            }
            let (source, sink) = if end == End::Source { (&me, &other) } else { (&other, &me) };
            shared.log(format!("a transfer from {source} to {sink} started here"));
            let carried = crate::data::splice(socket, peer).await;
            shared.log(format!("a transfer from {source} to {sink} ended; {carried} bytes came from {me}"));
        }
    }
}

/// The other end is not below this relay: this end goes up, on behalf of
/// `me`, and meets the other where both are below.
async fn carry(shared: &Shared, mut socket: Socket, signed: Signed, end: End, me: MachineName) {
    let endpoint = shared.state().up.as_ref().map(|up| up.endpoint.clone());
    let Some(endpoint) = endpoint else {
        refuse(&mut socket, shared.not_connected()).await;
        return;
    };
    let limit = wire::MAX_DATA_MESSAGE + wire::MAX_CONTROL_FRAME;
    let dialed = match dial::dial(&endpoint.addresses, endpoint.cert, wire::DATA_PATH, limit).await {
        Ok(dialed) => dialed,
        Err(error) => {
            let reason = format!("the relay {} could not reach {}: {error}", shared.me, endpoint.upstream);
            refuse(&mut socket, Refusal::Broken { reason }).await;
            return;
        }
    };
    let mut upper = dialed.socket;
    let hello = Up::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        intent: Intent::Data {
            key: shared.key,
            proof: shared.machine.prove(&dialed.binding),
            ticket: signed,
            end,
            on_behalf: Some(me),
        },
    };
    if let Err(error) = dial::send(&mut upper, &hello).await {
        refuse(&mut socket, Refusal::Broken { reason: format!("the relay {}: {error}", shared.me) }).await;
        return;
    }
    let answer = tokio::select! {
        answer = dial::recv(&mut upper) => answer,
        // The end below gave up or vanished.
        _ = socket.next() => {
            let _ = upper.close(None).await;
            return;
        }
    };
    match answer {
        Ok(Some(Down::Paired)) => {
            if send(&mut socket, &Down::Paired).await.is_ok() {
                crate::data::splice(socket, upper).await;
            }
        }
        Ok(Some(Down::Refused { reason })) => refuse(&mut socket, reason).await,
        Ok(Some(_)) => {
            let reason = "the upstream answered something other than a pairing".to_string();
            refuse(&mut socket, Refusal::Broken { reason }).await;
        }
        Ok(None) | Err(_) => {
            let reason = "the upstream closed the data connection".to_string();
            refuse(&mut socket, Refusal::Broken { reason }).await;
        }
    }
}
