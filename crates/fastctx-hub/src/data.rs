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
//! is in flight per direction, so the slower side sets the pace, and none
//! may take longer than the silence to pass on.
//!
//! An end waits for the other a minute at most, and less once the call it
//! goes with is over: its caller gave it up, or the machine that named it
//! left the hub (`forget`). How many ends may wait and how many pairs may
//! be relayed at once is bounded, for each member and for the hub, so one
//! machine gone wrong cannot fill the hub with transfers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{Down, End, Refusal, SILENCE, Ticket};
use fastctx_types::MachineName;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::oneshot;

use crate::control::Standing;
use crate::{Hub, Socket, refuse, send};

/// How long an end waits for the other to come.
const PAIRING: Duration = Duration::from_secs(60);

/// How many ends of one member may wait for the other at once.
const WAITING_FOR_ONE: usize = 256;
/// How many ends may wait at once in all.
const WAITING: usize = 8192;
/// How many pairs the hub relays at once.
const LIVE: usize = 8192;

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
    /// Tells the waiting end why it waits no longer.
    called_off: oneshot::Sender<String>,
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

/// The call the data connection `stream` goes with is over before its ends
/// were paired: an end waiting for the other is told `why`, and the
/// connection is no longer registered. A pair already relayed goes on until
/// its ends close it, as they do once their call is over.
pub(crate) fn forget(hub: &Hub, stream: u64, why: &str) {
    let mut pairing = hub.data.lock().expect("never poisoned");
    if pairing.live.contains_key(&stream) {
        return;
    }
    pairing.registered.remove(&stream);
    if let Some(waiting) = pairing.waiting.remove(&stream) {
        let _ = waiting.called_off.send(why.to_string());
    }
}

/// Forgets what the session `generation` of `name` registered and nobody
/// paired yet; an end waiting for the other end of one is told so.
pub(crate) fn member_left(hub: &Hub, name: &MachineName, generation: u64) {
    let mut pairing = hub.data.lock().expect("never poisoned");
    let Pairing { registered, live, waiting } = &mut *pairing;
    registered.retain(|stream, registration| {
        registration.by.retain(|by| *by != (name.clone(), generation));
        let kept = live.contains_key(stream) || !registration.by.is_empty();
        if !kept && let Some(waiting) = waiting.remove(stream) {
            let _ = waiting.called_off.send(format!("{name}, whose call it goes with, left the hub"));
        }
        kept
    });
}

/// Cuts every pair whose standing lapsed, and calls off the ends waiting
/// for a connection whose call lapsed.
pub(crate) fn cut_streams(hub: &Hub) {
    let standings: Vec<(u64, Vec<Standing>)> = {
        let pairing = hub.data.lock().expect("never poisoned");
        pairing
            .live
            .iter()
            .map(|(stream, live)| (*stream, live.standings.clone()))
            .chain(pairing.registered.iter().map(|(stream, registration)| (*stream, registration.standings.clone())))
            .collect()
    };
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
        } else if !pairing.live.contains_key(&stream) {
            pairing.registered.remove(&stream);
            if let Some(waiting) = pairing.waiting.remove(&stream) {
                let _ = waiting.called_off.send(format!("the right its call was made under was taken back: {why}"));
            }
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
    let (hand_over, mut handed) = oneshot::channel();
    let (called_off, mut calling_off) = oneshot::channel();
    let waiting = {
        let mut pairing = hub.data.lock().expect("never poisoned");
        match pairing.waiting.remove(&stream) {
            Some(found) => Ok(Some(found)),
            None => {
                let mine = pairing.waiting.values().filter(|waiting| waiting.member == me).count();
                if pairing.waiting.len() >= WAITING || mine >= WAITING_FOR_ONE {
                    Err(format!(
                        "the hub has {} data connections waiting for their other end; try again",
                        pairing.waiting.len()
                    ))
                } else {
                    pairing.waiting.insert(stream, Waiting { end, member: me.clone(), hand_over, called_off });
                    Ok(None)
                }
            }
        }
    };
    match waiting {
        Err(reason) => {
            hub.log.line(format_args!("refused a data connection of {me}: {reason}"));
            refuse(&mut socket, Refusal::Broken { reason }).await;
        }
        Ok(Some(waiting)) => {
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
        Ok(None) => {
            let peer = tokio::select! {
                handed = &mut handed => handed.map_err(|_| "the other end left".to_string()),
                // The waiting end gave up or vanished.
                _ = socket.next() => Err("this end left".to_string()),
                () = tokio::time::sleep(PAIRING) => {
                    Err(format!("the other end did not come within {} s", PAIRING.as_secs()))
                }
                Ok(why) = &mut calling_off => Err(why),
            };
            // A pairing that came meanwhile hands its end over still.
            let peer = match peer {
                Err(why) => {
                    let mut pairing = hub.data.lock().expect("never poisoned");
                    pairing.waiting.remove(&stream);
                    drop(pairing);
                    handed.try_recv().map_err(|_| why)
                }
                handed => handed,
            };
            match peer {
                Ok((peer, cutting)) => {
                    let carried = relay(socket, peer, cutting).await;
                    let mut pairing = hub.data.lock().expect("never poisoned");
                    pairing.live.remove(&stream);
                    pairing.registered.remove(&stream);
                    drop(pairing);
                    hub.log.line(format_args!("a transfer ended; {carried} bytes came from {me}"));
                }
                Err(reason) => {
                    refuse(&mut socket, Refusal::Broken { reason }).await;
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
        if pairing.live.len() >= LIVE {
            return Err(format!("the hub relays {} transfers already; try again", pairing.live.len()));
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
/// returns how many bytes passed from the first end to the second. An end
/// that does not take a message within the silence ends the pair, as one
/// that says nothing ends it at its own end.
async fn relay(mut first: Socket, mut second: Socket, cutting: oneshot::Receiver<String>) -> u64 {
    if !paired(&mut first).await || !paired(&mut second).await {
        return 0;
    }
    let mut carried = 0_u64;
    let (mut first_sink, mut first_stream) = first.split();
    let (mut second_sink, mut second_stream) = second.split();
    let forward = async {
        while let Some(Ok(message)) = first_stream.next().await {
            carried += message.len() as u64;
            if message.is_close()
                || !tokio::time::timeout(SILENCE, second_sink.send(message)).await.is_ok_and(|sent| sent.is_ok())
            {
                break;
            }
        }
    };
    let backward = async {
        while let Some(Ok(message)) = second_stream.next().await {
            if message.is_close()
                || !tokio::time::timeout(SILENCE, first_sink.send(message)).await.is_ok_and(|sent| sent.is_ok())
            {
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
    let _ = tokio::time::timeout(SILENCE, first_sink.close()).await;
    let _ = tokio::time::timeout(SILENCE, second_sink.close()).await;
    carried
}

/// Tells an end it is paired; whether it took that within the silence.
async fn paired(socket: &mut Socket) -> bool {
    tokio::time::timeout(SILENCE, send(socket, &Down::Paired)).await.is_ok_and(|sent| sent.is_ok())
}
