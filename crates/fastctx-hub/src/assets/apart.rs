//! Frame bodies too large for the control connection (design §7.1). A
//! device stages such a body on a data connection before it asks the hub to
//! take the frame; a reply of frames names a ticket under which a data
//! connection brings, in order, the bodies that travel apart. Either waits
//! in memory a bounded while, under the device's key, within a room every
//! device shares and a share of it each (§7.2).

use super::objects::{self, DONE, RECEIPT_WITHIN};
use crate::updates::receipt_writer::Writer;
use crate::updates::transfer::{heard, put as write};
use crate::{Hub, Socket};
use fastctx_net::wire::HEARTBEAT;
use fastctx_types::MachineName;
use fastctx_types::assets::{APART_FRAME_BYTES, CHUNK_BYTES, ChunkHead, INLINE_FRAME_BYTES};
use futures_util::StreamExt as _;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// How long a body waits for its data connection, or, staged, for its frame.
const WAITS: Duration = Duration::from_secs(120);
/// The room the bodies waiting on the hub share, and one device's share.
const ROOM: usize = 128 << 20;
const DEVICE_ROOM: usize = 8 << 20;

enum Body {
    /// Room for a staged body, its data connection not here yet.
    Coming,
    /// A staged body coming on its data connection now: it waits on no
    /// clock, the connection's own pulse bounds it.
    Receiving,
    Staged(Arc<Vec<u8>>),
    /// A staged body its frame is being taken with now: one take at a time,
    /// so no more copies of it are made than the room counts for (AC04).
    Taking(Arc<Vec<u8>>),
    /// The bodies a reply of frames sends apart, in order, their data
    /// connection not here yet.
    Going(Vec<Vec<u8>>),
    /// Bodies going on their data connection now: still counted in the room
    /// until it ends.
    Sending,
}

struct Waiting {
    key: [u8; 32],
    bytes: usize,
    at: Instant,
    body: Body,
}

/// The bodies waiting, by the ticket they travel under.
#[derive(Default)]
pub(crate) struct Apart {
    waiting: Mutex<HashMap<u64, Waiting>>,
}

impl Apart {
    fn reserve(&self, ticket: u64, key: [u8; 32], bytes: usize, body: Body) -> Result<(), String> {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        waiting.retain(|_, kept| matches!(kept.body, Body::Receiving | Body::Sending | Body::Taking(_)) || kept.at.elapsed() < WAITS);
        let total: usize = waiting.values().map(|kept| kept.bytes).sum();
        let device: usize = waiting.values().filter(|kept| kept.key == key).map(|kept| kept.bytes).sum();
        if total + bytes > ROOM || device + bytes > DEVICE_ROOM {
            return Err("the hub has no room for more frame bodies now".into());
        }
        waiting.insert(ticket, Waiting { key, bytes, at: Instant::now(), body });
        Ok(())
    }

    /// The staged body under `ticket` is coming now.
    fn receiving(&self, ticket: u64, key: &[u8; 32]) -> Result<(), String> {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        match waiting.get_mut(&ticket) {
            Some(kept) if kept.key == *key && matches!(kept.body, Body::Coming) => {
                kept.body = Body::Receiving;
                Ok(())
            }
            _ => Err("the room for the body went".into()),
        }
    }

    /// Lets the room under `ticket` go, whatever it held.
    fn forget(&self, ticket: u64, key: &[u8; 32]) {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        if waiting.get(&ticket).is_some_and(|kept| kept.key == *key) {
            waiting.remove(&ticket);
        }
    }

    pub(super) fn staged(&self, ticket: u64, key: &[u8; 32], bytes: Vec<u8>) -> Result<(), String> {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        match waiting.get_mut(&ticket) {
            Some(kept) if kept.key == *key && matches!(kept.body, Body::Coming | Body::Receiving) => {
                kept.body = Body::Staged(Arc::new(bytes));
                kept.at = Instant::now();
                Ok(())
            }
            _ => Err("the room for the body went".into()),
        }
    }

    /// The body the device whose key is `key` staged under `ticket`, for its
    /// frame to be taken with now: kept for the frame asked again until
    /// `release`, and given to one take at a time (`untake`, `touch`).
    pub(crate) fn body(&self, ticket: u64, key: &[u8; 32]) -> Result<Arc<Vec<u8>>, String> {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        match waiting.get_mut(&ticket) {
            Some(kept) if kept.key == *key && matches!(kept.body, Body::Taking(_)) => {
                Err("the frame staged under that ticket is being taken now: ask again".into())
            }
            Some(kept) if kept.key == *key && kept.at.elapsed() < WAITS => match &kept.body {
                Body::Staged(body) => {
                    let body = body.clone();
                    kept.body = Body::Taking(body.clone());
                    Ok(body)
                }
                _ => Err("no body is staged here under that ticket".into()),
            },
            _ => Err("no body is staged here under that ticket".into()),
        }
    }

    /// The take of the frame staged under `ticket` ended without a word on
    /// it: the body waits for the frame asked again.
    pub(crate) fn untake(&self, ticket: u64, key: &[u8; 32]) {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        if let Some(kept) = waiting.get_mut(&ticket)
            && kept.key == *key
            && let Body::Taking(body) = &kept.body
        {
            kept.body = Body::Staged(body.clone());
        }
    }

    /// Every body the device whose key is `key` staged waits on: it puts an
    /// object its frame waits for.
    pub(crate) fn keep_staged(&self, key: &[u8; 32]) {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        for kept in waiting.values_mut().filter(|kept| kept.key == *key && matches!(kept.body, Body::Staged(_))) {
            kept.at = Instant::now();
        }
    }

    /// The staged body under `ticket` waits on: its frame waits for objects.
    pub(crate) fn touch(&self, ticket: u64, key: &[u8; 32]) {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        if let Some(kept) = waiting.get_mut(&ticket)
            && kept.key == *key
        {
            match &kept.body {
                Body::Staged(_) => kept.at = Instant::now(),
                Body::Taking(body) => {
                    kept.body = Body::Staged(body.clone());
                    kept.at = Instant::now();
                }
                _ => {}
            }
        }
    }

    /// Lets the staged body under `ticket` go: its frame is settled.
    pub(crate) fn release(&self, ticket: u64, key: &[u8; 32]) {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        if waiting.get(&ticket).is_some_and(|kept| kept.key == *key) {
            waiting.remove(&ticket);
        }
    }

    /// The bodies kept under `ticket`, to go now: they stay counted in the
    /// room until they went (`forget`).
    pub(super) fn going(&self, ticket: u64, key: &[u8; 32]) -> Option<Vec<Vec<u8>>> {
        let mut waiting = self.waiting.lock().expect("bodies apart");
        match waiting.get_mut(&ticket) {
            Some(kept) if kept.key == *key && matches!(kept.body, Body::Going(_)) => {
                match std::mem::replace(&mut kept.body, Body::Sending) {
                    Body::Going(bodies) => Some(bodies),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// Room for a staged body of `size` bytes under `ticket`.
pub(super) fn stage(apart: &Apart, ticket: u64, key: &[u8; 32], size: u64) -> Result<(), String> {
    if size <= INLINE_FRAME_BYTES as u64 || size > APART_FRAME_BYTES {
        return Err(format!("a body staged apart is over {INLINE_FRAME_BYTES} bytes and at most {APART_FRAME_BYTES}"));
    }
    apart.reserve(ticket, *key, size as usize, Body::Coming)
}

/// Keeps `bodies`, in order, for a data connection under `ticket`.
pub(super) fn send_later(apart: &Apart, ticket: u64, key: &[u8; 32], bodies: Vec<Vec<u8>>) -> Result<(), String> {
    let bytes = bodies.iter().map(Vec::len).sum();
    apart.reserve(ticket, *key, bytes, Body::Going(bodies))
}

/// Takes a staged body of `size` bytes, its chunks in order, and says
/// whether the hub kept it.
pub(super) async fn receive(hub: &Arc<Hub>, mut socket: Socket, me: &MachineName, key: &[u8; 32], ticket: u64, size: u64) -> Result<(), String> {
    let pulse = crate::pulse_of(&socket);
    let apart = &hub.assets.objects.apart;
    let kept: Result<(), String> = async {
        apart.receiving(ticket, key)?;
        let mut body = Vec::with_capacity(size as usize);
        let mut expected = 0;
        while (body.len() as u64) < size {
            // No chunk longer than what is still to come.
            let (index, bytes) = objects::chunk_at_most(&mut socket, &pulse, size - body.len() as u64).await?;
            if index != expected {
                return Err("a body's chunks come in order".into());
            }
            expected += 1;
            body.extend_from_slice(&bytes);
        }
        if body.len() as u64 != size {
            return Err("the body came longer than it was staged as".into());
        }
        apart.staged(ticket, key, body)
    }
    .await;
    if kept.is_err() {
        apart.forget(ticket, key);
    }
    let (sink, _) = socket.split();
    let writer = Writer::new(sink, pulse, fastctx_net::data::ReceiptWork::default());
    let word = match &kept {
        Ok(()) => DONE.to_string(),
        Err(why) => serde_json::to_string(&format!("the hub did not keep the body: {why}")).expect("text serializes"),
    };
    if let Err(why) = writer.finish(word).await {
        hub.log.line(format_args!("{me}: sending a staged body's receipt: {why}"));
    }
    kept
}

/// Sends the bodies kept under `ticket`, in order, each in chunks, and
/// waits for the member's word that it has them all. Before each body the
/// machine is judged again, as it is then, for whether it still holds the
/// stream (§6.4, AC07): access ended meanwhile, the rest does not go.
pub(super) async fn send(
    hub: &Arc<Hub>,
    socket: Socket,
    me: &MachineName,
    public: &fastctx_net::sign::PublicKey,
    scope: &super::scopes::Scope,
    held: &str,
    ticket: u64,
) -> Result<(), String> {
    let key = public.as_bytes();
    let bodies = hub.assets.objects.apart.going(ticket, key).ok_or("the bodies under that ticket went")?;
    // Counted in the room until they went, however that ends.
    struct Gone<'a>(&'a Apart, u64, [u8; 32]);
    impl Drop for Gone<'_> {
        fn drop(&mut self) {
            self.0.forget(self.1, &self.2);
        }
    }
    let _gone = Gone(&hub.assets.objects.apart, ticket, *key);
    let pulse = crate::pulse_of(&socket);
    let (mut sink, mut stream) = socket.split();
    let (body_sent, sent) = tokio::sync::oneshot::channel();
    let receipt = async {
        let at = match sent.await {
            Ok(at) => at,
            Err(_) => return std::future::pending::<Result<(), String>>().await,
        };
        tokio::time::sleep_until(at + RECEIPT_WITHIN).await;
        Err(format!("the member did not say it had the bodies within {} s", RECEIPT_WITHIN.as_secs()))
    };
    let writer = async {
        let mut pace = fastctx_net::data::Pace::default();
        let mut index = 0u64;
        for body in &bodies {
            objects::still_holding(hub, me, public, scope, None, Some(held)).await?;
            for chunk in body.chunks(CHUNK_BYTES as usize) {
                let head = serde_json::to_string(&ChunkHead { index, size: chunk.len() as u64 }).expect("a head serializes");
                write(&mut sink, &pulse, WsMessage::Text(head.into())).await?;
                for piece in chunk.chunks(pace.chunk()) {
                    let began = std::time::Instant::now();
                    write(&mut sink, &pulse, WsMessage::Binary(piece.to_vec().into())).await?;
                    pace.wrote(piece.len(), began.elapsed());
                }
                index += 1;
            }
        }
        let _ = body_sent.send(tokio::time::Instant::now());
        loop {
            tokio::time::sleep(HEARTBEAT).await;
            write(&mut sink, &pulse, fastctx_net::pulse::ping(&pulse)).await?;
        }
    };
    let reader = async {
        loop {
            match heard(&mut stream, &pulse).await? {
                Some(WsMessage::Text(text)) if text.as_str() == DONE => return Ok(()),
                Some(WsMessage::Close(_)) | None => return Err("the member left before it had the bodies".to_string()),
                Some(WsMessage::Ping(said)) => {
                    if let Some(took) = fastctx_net::pulse::said(&said) {
                        pulse.told(took);
                    }
                }
                Some(_) => {}
            }
        }
    };
    tokio::select! {
        ended = reader => ended,
        ended = writer => ended,
        ended = receipt => ended,
    }
}
