//! One direct link: dialing it and keeping it, and everything that passes
//! on it until it ends. Either end calls the other over it, answers the
//! other's calls, and sends a heartbeat; three heartbeats unanswered and it
//! is taken for dead, as the line to the hub is.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use fastctx_types::access::Purpose;
use fastctx_types::{AccountName, MachineName};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::super::{CallEvent, CallOn, Inner};
use crate::again::{STABLE, backoff};
use crate::directory::View;
use crate::peer::{self, Frame, Intent};
use crate::sign::{Domain, PublicKey};
use crate::transport::Socket;
use crate::wire::{CallStatus, HEARTBEAT, SILENCE};

/// One live direct link.
pub(crate) struct PeerConn {
    pub name: MachineName,
    /// The key it proved it holds.
    pub key: PublicKey,
    /// Tells this link apart from earlier and later ones with the same
    /// machine.
    pub generation: u64,
    /// Whether this machine dialed it; that end dials the data connections
    /// that go with calls over it.
    pub dialed: bool,
    /// Where the other machine was reached, when this one dialed it.
    pub at: Vec<SocketAddr>,
    out: mpsc::UnboundedSender<Frame>,
    ms: Mutex<Option<u32>>,
    closed: watch::Sender<Option<String>>,
}

impl PeerConn {
    fn new(
        name: MachineName,
        key: PublicKey,
        dialed: bool,
        at: Vec<SocketAddr>,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<Frame>) {
        let (out, frames) = mpsc::unbounded_channel();
        let (closed, _) = watch::channel(None);
        let conn = Self {
            name,
            key,
            generation: super::super::ways::connection_number(),
            dialed,
            at,
            out,
            ms: Mutex::new(None),
            closed,
        };
        (Arc::new(conn), frames)
    }

    pub fn send(&self, frame: Frame) -> Result<(), ()> {
        self.out.send(frame).map_err(|_| ())
    }

    /// The round trip as last measured.
    pub fn ms(&self) -> Option<u32> {
        *self.ms.lock().expect("never poisoned")
    }

    /// Ends the link, saying why.
    pub fn close(&self, reason: String) {
        self.closed.send_if_modified(|closed| {
            if closed.is_some() {
                return false;
            }
            *closed = Some(reason);
            true
        });
    }

    /// Resolves once the link ended, with why.
    pub async fn ended(&self) -> String {
        let mut closed = self.closed.subscribe();
        loop {
            if let Some(reason) = closed.borrow_and_update().clone() {
                return reason;
            }
            if closed.changed().await.is_err() {
                return "the link ended".to_string();
            }
        }
    }

    pub fn on(&self) -> CallOn {
        CallOn::Peer(self.name.clone(), self.generation)
    }
}

/// A call another machine made over a direct link, while this one answers
/// it: what it was allowed under, and how to cut it when that is taken back.
pub(crate) struct Answering {
    conn: std::sync::Weak<PeerConn>,
    id: u64,
    account: AccountName,
    purpose: Purpose,
    cut: watch::Sender<Option<String>>,
}

/// Keeps a call being answered in the register until it is answered.
pub(crate) struct AnsweringGuard {
    inner: std::sync::Weak<Inner>,
    key: u64,
    /// Says why, when the call is cut.
    pub cut: watch::Receiver<Option<String>>,
}

impl AnsweringGuard {
    /// Whether the call's answer is still to be sent: it was not cut.
    pub fn wanted(&self) -> bool {
        self.cut.borrow().is_none()
    }
}

impl Drop for AnsweringGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.direct.answering.lock().expect("never poisoned").remove(&self.key);
        }
    }
}

impl Inner {
    /// Registers the call `id` that `conn`'s machine made for `account` and
    /// `purpose`, while this machine answers it.
    pub(crate) fn answering(
        self: &Arc<Self>,
        conn: &Arc<PeerConn>,
        id: u64,
        account: AccountName,
        purpose: Purpose,
    ) -> AnsweringGuard {
        let key = self.next_id();
        let (cut, receiver) = watch::channel(None);
        let answering = Answering { conn: Arc::downgrade(conn), id, account, purpose, cut };
        self.direct.answering.lock().expect("never poisoned").insert(key, answering);
        AnsweringGuard { inner: Arc::downgrade(self), key, cut: receiver }
    }

    /// Cuts every call being answered over a direct link whose right the
    /// roster no longer gives, as the hub cuts the calls it passes on: the
    /// caller is told, and the call's data connections end.
    pub(crate) fn cut_lapsed(&self, view: &View) {
        let me = &view.me;
        let mut answering = self.direct.answering.lock().expect("never poisoned");
        let lapsed: Vec<(u64, String)> = answering
            .iter()
            .filter_map(|(key, call)| {
                let from = call.conn.upgrade().map(|conn| conn.name.clone())?;
                view.admits(&from, &call.account, me, call.purpose).err().map(|why| (*key, why))
            })
            .collect();
        for (key, why) in lapsed {
            let Some(call) = answering.remove(&key) else { continue };
            let reason = format!("the right it was made under was taken back: {why}");
            if let Some(conn) = call.conn.upgrade() {
                self.node.log(format!("cut a direct call from {}: {why}", conn.name));
                let _ = conn.send(Frame::Cut { id: call.id, reason: reason.clone() });
            }
            call.cut.send_replace(Some(reason));
        }
    }
}

/// Keeps a direct link with `peer` at `at`: dials it, and dials again
/// whenever it drops or cannot be made, leaving a line for every try.
pub(super) async fn keep(inner: Arc<Inner>, peer: MachineName, at: Vec<SocketAddr>) {
    let mut failures: u32 = 0;
    let mut network = inner.network.subscribe();
    loop {
        let started = Instant::now();
        let why = match dial(&inner, &peer, &at).await {
            Ok((conn, socket, frames, address, route)) => {
                inner.node.log(format!("a direct link with {peer} is up, to {address} {route}"));
                inner.direct_up(&conn);
                let (reason, moved) = serve(inner.clone(), conn.clone(), socket, frames, Some(&mut network)).await;
                inner.direct_down(&conn, &reason);
                if moved || started.elapsed() >= STABLE {
                    failures = 0;
                }
                format!("the link went down: {reason}")
            }
            Err(why) => why,
        };
        failures = failures.saturating_add(1);
        let wait = backoff(failures);
        let next = if wait.is_zero() {
            "trying again at once".to_string()
        } else {
            format!("trying again in {:.1} s", wait.as_secs_f64())
        };
        inner.node.log(format!(
            "no direct link with {peer} (try {failures}, which took {:.1} s): {why}; what goes to it goes through the hub; {next}",
            started.elapsed().as_secs_f64()
        ));
        inner.direct.mark(&peer, false, Some(why));
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            changed = network.changed() => {
                if changed.is_err() {
                    return;
                }
                failures = 0;
            }
        }
    }
}

type Linked = (Arc<PeerConn>, Socket, mpsc::UnboundedReceiver<Frame>, SocketAddr, fastctx_types::world::Route);

/// Dials `peer` at `at` and has it prove who it is.
async fn dial(inner: &Inner, peer: &MachineName, at: &[SocketAddr]) -> Result<Linked, String> {
    let key =
        inner.view().member(peer).map(|member| member.key).ok_or_else(|| format!("{peer} is not in the roster"))?;
    let dialed = crate::dial::dial_peer(at, peer::LINK_PATH, crate::wire::MAX_CONTROL_FRAME).await?;
    let mut socket = dialed.socket;
    let hello = Frame::Hello {
        protocol: peer::PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        hub: inner.hub.clone(),
        intent: Intent::Link { key: inner.key.public(), proof: inner.key.sign(Domain::Peer, &dialed.binding) },
    };
    peer::send(&mut socket, &hello).await?;
    match peer::next(&mut socket).await? {
        Frame::Welcome { name, proof } => {
            if name != *peer {
                return Err(format!("{name} answered at {}, not {peer}", dialed.address));
            }
            key.verify(Domain::Peer, &dialed.binding, &proof)
                .map_err(|_| format!("what answered at {} does not hold {peer}'s key", dialed.address))?;
        }
        Frame::Refused { reason } => return Err(format!("{peer} refused the link: {reason}")),
        _ => return Err(format!("{peer} answered the link with something other than a welcome")),
    }
    let (conn, frames) = PeerConn::new(peer.clone(), key, true, at.to_vec());
    Ok((conn, socket, frames, dialed.address, dialed.route))
}

/// A link another machine opened to this one, once it proved who it is.
pub(super) async fn taken(inner: Arc<Inner>, name: MachineName, key: PublicKey, socket: Socket, from: SocketAddr) {
    let (conn, frames) = PeerConn::new(name.clone(), key, false, Vec::new());
    inner.node.log(format!("a direct link from {name} came in from {from}"));
    inner.direct_up(&conn);
    // The machine that dialled keeps the link and dials again, so a change
    // in this machine's network does not end it here; its silence does.
    let (reason, _) = serve(inner.clone(), conn.clone(), socket, frames, None).await;
    inner.direct_down(&conn, &reason);
    inner.node.log(format!("the direct link from {name} ended: {reason}"));
}

impl Inner {
    fn direct_up(&self, conn: &Arc<PeerConn>) {
        let older = {
            let mut links = self.direct.links.lock().expect("never poisoned");
            let ends = links.entry(conn.name.clone()).or_default();
            let end = if conn.dialed { &mut ends.dialed } else { &mut ends.taken };
            end.replace(conn.clone())
        };
        if let Some(older) = older {
            older.close("a newer link with the same machine replaced it".to_string());
        }
        self.direct.mark(&conn.name, true, None);
        self.ways_changed();
    }

    fn direct_down(&self, conn: &Arc<PeerConn>, reason: &str) {
        let still_up = {
            let mut links = self.direct.links.lock().expect("never poisoned");
            if let Some(ends) = links.get_mut(&conn.name) {
                for end in [&mut ends.dialed, &mut ends.taken] {
                    if end.as_ref().is_some_and(|live| live.generation == conn.generation) {
                        *end = None;
                    }
                }
            }
            let up = links.get(&conn.name).is_some_and(|ends| ends.dialed.is_some() || ends.taken.is_some());
            if !up {
                links.remove(&conn.name);
            }
            up
        };
        if !still_up {
            self.direct.mark(&conn.name, false, Some(reason.to_string()));
        }
        // As with the hub's line: the way is gone before its calls hear so.
        self.ways_changed();
        self.fail_calls_on(&conn.on(), reason);
    }
}

/// Everything that passes on a link once both ends proved who they are,
/// until it ends; returns why it ended, and whether because this machine's
/// `network` changed, when the old way may no longer be there.
async fn serve(
    inner: Arc<Inner>,
    conn: Arc<PeerConn>,
    socket: Socket,
    mut frames: mpsc::UnboundedReceiver<Frame>,
    mut network: Option<&mut watch::Receiver<u64>>,
) -> (String, bool) {
    let (mut sink, mut stream) = socket.split();
    let (writer_done, mut writer_stopped) = tokio::sync::oneshot::channel::<()>();
    let (finish, mut finishing) = tokio::sync::oneshot::channel::<String>();
    // The writer outlives the link by as long as it takes to say why it
    // ended, so whoever ends it is not held up by a peer that went silent.
    tokio::spawn(async move {
        let _done = writer_done;
        loop {
            tokio::select! {
                frame = frames.recv() => match frame {
                    Some(frame) => {
                        if sink.send(WsMessage::Binary(peer::encode(&frame).into())).await.is_err() {
                            return;
                        }
                    }
                    None => break,
                },
                reason = &mut finishing => {
                    if let Ok(reason) = reason {
                        let refusal = WsMessage::Binary(peer::encode(&Frame::Refused { reason }).into());
                        let _ = tokio::time::timeout(SILENCE, sink.send(refusal)).await;
                    }
                    break;
                }
            }
        }
        let _ = tokio::time::timeout(SILENCE, sink.close()).await;
    });
    let pinged: Arc<Mutex<HashMap<u64, Instant>>> = Arc::default();
    let heartbeat = {
        let (conn, pinged) = (conn.clone(), pinged.clone());
        tokio::spawn(async move {
            let mut n = 0;
            loop {
                tokio::time::sleep(HEARTBEAT).await;
                n += 1;
                pinged.lock().expect("never poisoned").insert(n, Instant::now());
                if conn.send(Frame::Ping { n }).is_err() {
                    return;
                }
            }
        })
    };
    let ended = conn.clone();
    let ended = ended.ended();
    tokio::pin!(ended);
    // Anything the other machine sends shows it is there, a pong or not.
    let mut heard = Instant::now();
    let mut last_ping = 0;
    if let Some(network) = network.as_mut() {
        network.mark_unchanged();
    }
    let mut moved = false;
    let reason = loop {
        let next = tokio::select! {
            biased;
            () = tokio::time::sleep_until(heard + SILENCE) => {
                break format!("nothing came from it for {} s", SILENCE.as_secs());
            }
            reason = &mut ended => break reason,
            _ = &mut writer_stopped => break "writing to it failed".to_string(),
            changed = network_changed(&mut network) => {
                moved = changed;
                break "this machine's network changed".to_string();
            }
            next = stream.next() => next,
        };
        let frame: Frame = match next {
            None | Some(Ok(WsMessage::Close(_))) => break "it closed the link".to_string(),
            Some(Err(error)) => break crate::dial::describe(&error),
            Some(Ok(WsMessage::Binary(bytes))) => match serde_json::from_slice(&bytes) {
                Ok(frame) => frame,
                Err(error) => break format!("an unreadable frame from it: {error}"),
            },
            Some(Ok(_)) => {
                heard = Instant::now();
                continue;
            }
        };
        heard = Instant::now();
        match frame {
            Frame::Ping { n } => {
                let _ = conn.send(Frame::Pong { n });
            }
            Frame::Pong { n } => {
                if n > last_ping {
                    last_ping = n;
                    if let Some(sent) = pinged.lock().expect("never poisoned").remove(&n) {
                        let ms = u32::try_from(sent.elapsed().as_millis()).unwrap_or(u32::MAX);
                        *conn.ms.lock().expect("never poisoned") = Some(ms);
                    }
                    pinged.lock().expect("never poisoned").retain(|sent, _| *sent > n);
                }
            }
            Frame::Call { id, message } => {
                tokio::spawn(super::super::answering::answer(
                    inner.clone(),
                    super::super::Replier::Peer(conn.clone(), id),
                    message,
                ));
            }
            Frame::Answer { id, message } => inner.call_event(&conn.on(), id, CallEvent::Answered(message)),
            Frame::Cut { id, reason } => {
                inner.call_event(&conn.on(), id, CallEvent::Status(CallStatus::Cut { reason }));
            }
            Frame::Refused { reason } => break format!("it ended the link: {reason}"),
            Frame::Hello { .. } | Frame::Welcome { .. } => {
                let _ = conn.send(Frame::Refused { reason: "a second hello on one link".to_string() });
                break "it said hello again on a link already open".to_string();
            }
        }
    };
    heartbeat.abort();
    conn.close(reason.clone());
    let _ = finish.send(reason.clone());
    (reason, moved)
}

/// Whether `network` changed, once it does; never without one, and false
/// when the node it belongs to is going away.
async fn network_changed(network: &mut Option<&mut watch::Receiver<u64>>) -> bool {
    match network {
        Some(network) => network.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}
