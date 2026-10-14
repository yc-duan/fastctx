//! Data connections: one per transfer, paired by the hub and relayed message
//! for message, so a large transfer never holds up the heartbeats, calls and
//! letters on the control connections.
//!
//! A call or an answer that comes with a data connection names it
//! (`fastctx_net::wire::Ticket`), and the hub registers it then, with what
//! that call was allowed under. Each end connects with the stream's number;
//! the first to arrive waits for the other, the second hands its connection
//! to the first, which relays in both directions until either side ends.
//! The hub pairs two ends only when both are the machines the registration
//! names, and only while what the call was allowed under still holds; when
//! the directory takes it back, the pair is cut (`cut_streams`). One message
//! is in flight per direction, so the slower side sets the pace.

use std::collections::HashMap;
use std::sync::Arc;

use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{Down, End, Refusal, Ticket};
use fastctx_types::MachineName;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::oneshot;

use crate::control::Standing;
use crate::{Hub, Socket, refuse, send};

#[derive(Default)]
pub(crate) struct Pairing {
    registered: HashMap<u64, Registration>,
    waiting: HashMap<u64, Waiting>,
    live: HashMap<u64, Live>,
}

struct Registration {
    source: MachineName,
    sink: MachineName,
    standings: Vec<Standing>,
    /// The sessions of the members whose calls or answers named it.
    by: Vec<(MachineName, u64)>,
}

struct Waiting {
    end: End,
    member: MachineName,
    hand_over: oneshot::Sender<(Socket, oneshot::Receiver<String>)>,
}

struct Live {
    standings: Vec<Standing>,
    cut: Option<oneshot::Sender<String>>,
}

/// Notes the data connection `ticket` names, allowed under `standing`, as
/// the session `generation` of `by` asked.
pub(crate) fn register(hub: &Hub, ticket: &Ticket, standing: &Standing, by: &MachineName, generation: u64) {
    // Its ends are known by their names now: the members that come to pair
    // it are.
    let (source, sink) = {
        let store = hub.store();
        let directory = &store.book.directory;
        (directory.canonical(&ticket.source).clone(), directory.canonical(&ticket.sink).clone())
    };
    let mut pairing = hub.data.lock().expect("never poisoned");
    let registration = pairing.registered.entry(ticket.stream).or_insert_with(|| Registration {
        source: source.clone(),
        sink: sink.clone(),
        standings: Vec::new(),
        by: Vec::new(),
    });
    if registration.source != source || registration.sink != sink {
        drop(pairing);
        hub.log.line(format_args!(
            "two calls named one data connection between different machines; the second was not registered"
        ));
        return;
    }
    if !registration.standings.contains(standing) {
        registration.standings.push(standing.clone());
    }
    registration.by.push((by.clone(), generation));
}

/// Forgets what the session `generation` of `name` registered and nobody
/// paired yet.
pub(crate) fn member_left(hub: &Hub, name: &MachineName, generation: u64) {
    let mut pairing = hub.data.lock().expect("never poisoned");
    let Pairing { registered, live, .. } = &mut *pairing;
    registered.retain(|stream, registration| {
        registration.by.retain(|by| *by != (name.clone(), generation));
        live.contains_key(stream) || !registration.by.is_empty()
    });
}

/// Cuts every pair whose standing lapsed.
pub(crate) fn cut_streams(hub: &Hub) {
    let standings: Vec<(u64, Vec<Standing>)> = hub
        .data
        .lock()
        .expect("never poisoned")
        .live
        .iter()
        .map(|(stream, live)| (*stream, live.standings.clone()))
        .collect();
    let lapsed: Vec<(u64, String)> = {
        let store = hub.store();
        let directory = &store.book.directory;
        standings
            .into_iter()
            .filter_map(|(stream, standings)| {
                standings
                    .iter()
                    .find_map(|standing| {
                        directory.admits(&standing.from, &standing.account, &standing.to, standing.purpose).err()
                    })
                    .map(|why| (stream, why))
            })
            .collect()
    };
    let mut pairing = hub.data.lock().expect("never poisoned");
    for (stream, why) in lapsed {
        if let Some(cut) = pairing.live.get_mut(&stream).and_then(|live| live.cut.take()) {
            let _ = cut.send(why);
        }
    }
}

pub(crate) async fn end(
    hub: Arc<Hub>,
    mut socket: Socket,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
    stream: u64,
    end: End,
) {
    let me = match hub.member(&key, &proof, &binding) {
        Ok(me) => me,
        Err(reason) => {
            hub.log.line(format_args!("refused a data connection: {reason}"));
            refuse(&mut socket, reason).await;
            return;
        }
    };
    // Taken out, or put in to wait, in one look: two ends that come at once
    // find each other.
    let (hand_over, handed) = oneshot::channel();
    let waiting = {
        let mut pairing = hub.data.lock().expect("never poisoned");
        let found = pairing.waiting.remove(&stream);
        if found.is_none() {
            pairing.waiting.insert(stream, Waiting { end, member: me.clone(), hand_over });
        }
        found
    };
    match waiting {
        Some(waiting) => {
            let (cut, cutting) = oneshot::channel();
            match pair(&hub, stream, &waiting, end, &me, cut) {
                Ok((source, sink)) => {
                    hub.log.line(format_args!("a transfer from {source} to {sink} started"));
                    // The first end relays; this one is handed over to it,
                    // with the means to cut the pair.
                    if let Err((mut socket, _)) = waiting.hand_over.send((socket, cutting)) {
                        hub.data.lock().expect("never poisoned").live.remove(&stream);
                        refuse(&mut socket, Refusal::Broken { reason: "the other end left".to_string() }).await;
                    }
                }
                Err(reason) => {
                    hub.log.line(format_args!("refused to pair a data connection of {me}: {reason}"));
                    refuse(&mut socket, Refusal::Broken { reason }).await;
                }
            }
        }
        None => {
            let peer = tokio::select! {
                handed = handed => handed.ok(),
                // The waiting end gave up or vanished.
                _ = socket.next() => None,
            };
            hub.data.lock().expect("never poisoned").waiting.remove(&stream);
            match peer {
                Some((peer, cutting)) => {
                    let carried = relay(socket, peer, cutting).await;
                    let mut pairing = hub.data.lock().expect("never poisoned");
                    pairing.live.remove(&stream);
                    pairing.registered.remove(&stream);
                    drop(pairing);
                    hub.log.line(format_args!("a transfer ended; {carried} bytes came from {me}"));
                }
                None => {
                    refuse(&mut socket, Refusal::Broken { reason: "the other end did not come".to_string() }).await;
                }
            }
        }
    }
}

/// Checks that the two ends of `stream` are the machines a registration
/// names and that what it was allowed under still holds; marks the pair
/// live. Returns the source and the sink.
fn pair(
    hub: &Hub,
    stream: u64,
    waiting: &Waiting,
    end: End,
    me: &MachineName,
    cut: oneshot::Sender<String>,
) -> Result<(MachineName, MachineName), String> {
    if waiting.end == end {
        return Err("two ends of one kind presented one stream".to_string());
    }
    let (source, sink) = match end {
        End::Source => (me.clone(), waiting.member.clone()),
        End::Sink => (waiting.member.clone(), me.clone()),
    };
    let standings = {
        let pairing = hub.data.lock().expect("never poisoned");
        let registration =
            pairing.registered.get(&stream).ok_or("no call the hub passed on named this data connection")?;
        if registration.source != source || registration.sink != sink {
            return Err(format!(
                "the data connection is for {} to {}, not {source} to {sink}",
                registration.source, registration.sink
            ));
        }
        registration.standings.clone()
    };
    let covers =
        |member: &MachineName| standings.iter().any(|standing| standing.from == *member || standing.to == *member);
    if !covers(&source) || !covers(&sink) {
        return Err(format!("no call allowed both {source} and {sink} on this data connection"));
    }
    {
        let store = hub.store();
        let directory = &store.book.directory;
        if let Some(why) = standings.iter().find_map(|standing| {
            directory.admits(&standing.from, &standing.account, &standing.to, standing.purpose).err()
        }) {
            return Err(why);
        }
    }
    hub.data.lock().expect("never poisoned").live.insert(stream, Live { standings, cut: Some(cut) });
    Ok((source, sink))
}

/// Relays between the two ends until either is done or the pair is cut;
/// returns how many bytes passed from the first end to the second.
async fn relay(mut first: Socket, mut second: Socket, cutting: oneshot::Receiver<String>) -> u64 {
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
    // A pair whose cut is dropped unsent ended some other way.
    let cut = async {
        if cutting.await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    // A WebSocket close does not pass through as half a close: when either
    // side is done, the transfer is.
    tokio::select! {
        () = forward => {}
        () = backward => {}
        () = cut => {}
    }
    let _ = first_sink.close().await;
    let _ = second_sink.close().await;
    carried
}
