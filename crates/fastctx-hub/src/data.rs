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
//! An end or a registration expires after a minute without pairing, even
//! when neither end comes. It waits less once its call is over or the
//! machine that named it left the hub (`forget`). A paired registration
//! belongs to the relay until its ends close. How many ends may wait and
//! how many pairs may be relayed at once is bounded, so one
//! machine gone wrong cannot fill the hub with transfers.

mod lifetime;

pub(crate) use lifetime::expire;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{Down, End, Refusal, SILENCE, Ticket};
use fastctx_types::MachineName;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::control::Standing;
use crate::{Hub, Socket, refuse, send};

/// How long a registration or an end waits for pairing.
const PAIRING: Duration = Duration::from_secs(60);

/// How many ends of one member may wait for the other at once: a screen
/// reading hundreds of files through the hub opens as many, and an end
/// pairs in moments; the totals below are what bound the hub's connections.
const WAITING_FOR_ONE: usize = 1024;
/// How many ends may wait at once in all.
const WAITING: usize = 8192;
/// How many pairs the hub relays at once.
const LIVE: usize = 8192;

#[derive(Default)]
pub(crate) struct Pairing {
    registered: HashMap<u64, Registration>,
    waiting: HashMap<u64, WaitingEnd>,
    live: HashMap<u64, Live>,
}

struct Registration {
    /// Distinguishes a new registration using the same stream number.
    serial: u64,
    /// Set by the first mention; later mentions do not extend the wait.
    deadline: Instant,
    source: MachineName,
    sink: MachineName,
    standings: Vec<Standing>,
    /// The sessions of the members whose calls or answers named it.
    by: Vec<(MachineName, u64)>,
}

#[derive(Clone)]
struct Waiting {
    /// Tells this end apart from any other that came with its number.
    serial: u64,
    end: End,
    member: MachineName,
}

struct WaitingEnd {
    waiting: Waiting,
    hand_over: oneshot::Sender<(Socket, oneshot::Receiver<String>)>,
    /// Tells the waiting end why it waits no longer.
    called_off: oneshot::Sender<String>,
}

struct Live {
    serial: u64,
    standings: Vec<Standing>,
    cut: Option<oneshot::Sender<String>>,
}

/// How many ends wait for the other, pairs are relayed, and connections
/// are registered, now.
pub(crate) fn counts(hub: &Hub) -> (usize, usize, usize) {
    let pairing = hub.data.lock().expect("never poisoned");
    (pairing.waiting.len(), pairing.live.len(), pairing.registered.len())
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
        serial: hub.next(),
        deadline: Instant::now() + PAIRING,
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
    let owner = (by.clone(), generation);
    if !registration.by.contains(&owner) {
        registration.by.push(owner);
    }
}

/// The call the data connection `stream` goes with is over before its ends
/// were paired: an end waiting for the other is told `why`, and the
/// connection is no longer registered. A pair already relayed goes on until
/// its ends close it, as they do once their call is over.
pub(crate) fn forget(hub: &Hub, stream: u64, why: &str) {
    hub.data.lock().expect("never poisoned").forget(stream, why);
}

impl Pairing {
    fn forget(&mut self, stream: u64, why: &str) {
        if self.live.contains_key(&stream) {
            return;
        }
        self.registered.remove(&stream);
        if let Some(waiting) = self.waiting.remove(&stream) {
            let _ = waiting.called_off.send(why.to_string());
        }
    }

    fn forget_waiting(&mut self, stream: u64, serial: u64, why: &str) -> bool {
        if self.waiting.get(&stream).is_some_and(|waiting| waiting.waiting.serial == serial) {
            self.forget(stream, why);
            true
        } else {
            false
        }
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
    // Keep the first end visible until handover commits, so cancellation
    // and expiry can still find it while the second checks the pair.
    let (hand_over, mut handed) = oneshot::channel();
    let (called_off, mut calling_off) = oneshot::channel();
    let serial = hub.next();
    let waiting = {
        let mut pairing = hub.data.lock().expect("never poisoned");
        if pairing.live.contains_key(&stream) {
            Err("this data connection is already paired".to_string())
        } else if let Some(found) = pairing.waiting.get(&stream) {
            let registration = pairing.registered.get(&stream).map(|registration| registration.serial);
            Ok(Some((found.waiting.clone(), registration)))
        } else {
            let mine = pairing.waiting.values().filter(|waiting| waiting.waiting.member == me).count();
            if pairing.waiting.len() >= WAITING || mine >= WAITING_FOR_ONE {
                let reason = format!(
                    "the hub has {} data connections waiting for their other end; try again",
                    pairing.waiting.len()
                );
                pairing.forget(stream, &reason);
                Err(reason)
            } else {
                let waiting = Waiting { serial, end, member: me.clone() };
                pairing.waiting.insert(stream, WaitingEnd { waiting, hand_over, called_off });
                Ok(None)
            }
        }
    };
    match waiting {
        Err(reason) => {
            hub.log.line(format_args!("refused a data connection of {me}: {reason}"));
            refuse(&mut socket, Refusal::Broken { reason }).await;
        }
        Ok(Some((waiting, registration))) => match pair(&hub, stream, &waiting, end, &me) {
            Ok((source, sink, standings)) => {
                // The first end relays. Capacity, ownership and handover
                // are decided together, before either end can give up.
                let (cut, cutting) = oneshot::channel();
                let handed = {
                    let mut pairing = hub.data.lock().expect("never poisoned");
                    let current =
                        pairing.waiting.get(&stream).is_some_and(|first| first.waiting.serial == waiting.serial);
                    let registered = pairing
                        .registered
                        .get(&stream)
                        .is_some_and(|found| Some(found.serial) == registration && found.deadline > Instant::now());
                    let refused = if !current {
                        Some("the other end is no longer waiting".to_string())
                    } else if !registered {
                        Some("the data connection's registration ended before pairing".to_string())
                    } else if pairing.live.len() >= LIVE {
                        Some(format!("the hub relays {} transfers already; try again", pairing.live.len()))
                    } else {
                        None
                    };
                    if let Some(reason) = refused {
                        pairing.forget_waiting(stream, waiting.serial, &reason);
                        Err((socket, reason))
                    } else {
                        let first = pairing.waiting.remove(&stream).expect("still waiting under this lock");
                        pairing.live.insert(stream, Live { serial: waiting.serial, standings, cut: Some(cut) });
                        first.hand_over.send((socket, cutting)).map_err(|(socket, _)| {
                            pairing.live.remove(&stream);
                            pairing.registered.remove(&stream);
                            (socket, "the other end left".to_string())
                        })
                    }
                };
                match handed {
                    Ok(()) => hub.log.line(format_args!("a transfer from {source} to {sink} started")),
                    Err((mut socket, reason)) => {
                        hub.log.line(format_args!("refused to pair a data connection of {me}: {reason}"));
                        refuse(&mut socket, Refusal::Broken { reason }).await;
                    }
                }
            }
            Err(reason) => {
                hub.data.lock().expect("never poisoned").forget_waiting(stream, waiting.serial, &reason);
                hub.log.line(format_args!("refused to pair a data connection of {me}: {reason}"));
                refuse(&mut socket, Refusal::Broken { reason }).await;
            }
        },
        Ok(None) => {
            let active = lifetime::Active::new(&hub, stream, serial);
            let peer = tokio::select! {
                handed = &mut handed => Ok(handed.map_err(|_| "the other end left".to_string())),
                // The waiting end gave up or vanished.
                _ = socket.next() => Err("this end left".to_string()),
                () = tokio::time::sleep(PAIRING) => {
                    Err(format!("the other end did not come within {} s", PAIRING.as_secs()))
                }
                Ok(why) = &mut calling_off => Err(why),
            };
            // A simultaneous handover wins only if it committed before the
            // wait was removed. Its sender was consumed under that lock.
            let peer = match peer {
                Err(why) => {
                    let still = hub.data.lock().expect("never poisoned").forget_waiting(stream, serial, &why);
                    if still { Err(why) } else { (&mut handed).await.map_err(|_| why) }
                }
                Ok(handed) => handed,
            };
            match peer {
                Ok((peer, cutting)) => {
                    let carried = relay(socket, peer, cutting).await;
                    drop(active);
                    hub.log.line(format_args!("a transfer ended; {carried} bytes came from {me}"));
                }
                Err(reason) => {
                    drop(active);
                    refuse(&mut socket, Refusal::Broken { reason }).await;
                }
            }
        }
    }
}

/// Checks that the two ends of `stream` are the machines a registration
/// names and that what it was allowed under still holds. Returns the source,
/// the sink, and what the pair was allowed under.
fn pair(
    hub: &Hub,
    stream: u64,
    waiting: &Waiting,
    end: End,
    me: &MachineName,
) -> Result<(MachineName, MachineName, Vec<Standing>), String> {
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
    Ok((source, sink, standings))
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
    let (first_pulse, second_pulse) = (crate::pulse_of(&first), crate::pulse_of(&second));
    let (mut first_sink, mut first_stream) = first.split();
    let (mut second_sink, mut second_stream) = second.split();
    // Each message is passed on for as long as it moves: an end's pings say
    // how much it took of what the hub sent it (`fastctx_net::pulse`), and
    // go on to the other end, which counts them as the far end's own. Each
    // direction ends with why, when the end it wrote to took nothing.
    let forward = async {
        while let Some(Ok(message)) = first_stream.next().await {
            told(&first_pulse, &message);
            carried += message.len() as u64;
            if message.is_close() {
                break;
            }
            match second_pulse.writing(second_sink.send(message)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(quiet) => return Some(quiet.to_string()),
            }
        }
        None
    };
    let backward = async {
        while let Some(Ok(message)) = second_stream.next().await {
            told(&second_pulse, &message);
            if message.is_close() {
                break;
            }
            match first_pulse.writing(first_sink.send(message)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(quiet) => return Some(quiet.to_string()),
            }
        }
        None
    };
    // A pair whose cut is dropped unsent ended some other way.
    let cut = async {
        if cutting.await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    // A WebSocket close does not pass through as half a close: when either
    // side is done, the transfer is. The end whose far end took nothing is
    // told so in the close, as its own write would have found out.
    let (first_why, second_why) = tokio::select! {
        why = forward => (why, None),
        why = backward => (None, why),
        () = cut => (None, None),
    };
    close(&mut first_sink, first_why).await;
    close(&mut second_sink, second_why).await;
    carried
}

/// Closes an end of a pair, saying `why` when there is something to say.
async fn close(sink: &mut futures_util::stream::SplitSink<Socket, crate::WsMessage>, why: Option<String>) {
    use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
    if let Some(why) = why {
        let frame = CloseFrame { code: CloseCode::Away, reason: why.into() };
        let _ = tokio::time::timeout(SILENCE, sink.send(crate::WsMessage::Close(Some(frame)))).await;
    }
    let _ = tokio::time::timeout(SILENCE, sink.close()).await;
}

/// Notes what an end's ping says it took of what the hub sent it.
fn told(pulse: &fastctx_net::pulse::Pulse, message: &crate::WsMessage) {
    if let crate::WsMessage::Ping(said) = message
        && let Some(took) = fastctx_net::pulse::said(said)
    {
        pulse.told(took);
    }
}

/// Tells an end it is paired; whether it took that within the silence.
async fn paired(socket: &mut Socket) -> bool {
    tokio::time::timeout(SILENCE, send(socket, &Down::Paired)).await.is_ok_and(|sent| sent.is_ok())
}
