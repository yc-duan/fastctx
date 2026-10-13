//! Data connections that go with calls over a direct link: straight between
//! the two machines, never through the hub. The machine that dialed the link
//! dials them too, since it is the one known to reach the other; the other
//! takes them on its listener and hands each to the call it belongs to,
//! whichever of the two comes first.

use std::sync::Arc;

use fastctx_types::MachineName;
use tokio::sync::{oneshot, watch};

use super::super::Inner;
use super::PeerConn;
use crate::peer::{self, Frame, Intent};
use crate::sign::Domain;
use crate::transport::Socket;
use crate::wire::{End, SILENCE};

/// A data connection another machine opened before the call here that it
/// goes with asked for it.
pub(crate) struct Arrived {
    /// The other machine's end.
    end: End,
    socket: Socket,
    number: u64,
}

/// A call here waiting for the other machine to open its data connection.
pub(crate) struct Awaited {
    /// This machine's end.
    end: End,
    hand: oneshot::Sender<Socket>,
}

/// How the data connections of one call over a direct link are made.
pub struct DataWay {
    inner: Arc<Inner>,
    conn: Arc<PeerConn>,
    cut: Option<watch::Receiver<Option<String>>>,
}

impl DataWay {
    /// The data connections of a call over `conn`; `cut`, for a call this
    /// machine answers, ends them when the call is cut.
    pub(crate) fn new(inner: Arc<Inner>, conn: Arc<PeerConn>, cut: Option<watch::Receiver<Option<String>>>) -> Self {
        Self { inner, conn, cut }
    }

    pub(crate) fn cut(&self) -> Option<watch::Receiver<Option<String>>> {
        self.cut.clone()
    }

    /// This machine's end `end` of the data connection `stream`.
    pub(crate) async fn open(&self, stream: u64, end: End) -> Result<Socket, String> {
        if self.conn.dialed { self.dial(stream, end).await } else { self.await_other(stream, end).await }
    }

    async fn dial(&self, stream: u64, end: End) -> Result<Socket, String> {
        let limit = crate::wire::MAX_DATA_MESSAGE + crate::wire::MAX_CONTROL_FRAME;
        // What went wrong names the other machine's addresses: it goes to
        // this machine's log, and the caller, which may be a model, learns
        // only which machine.
        let dialed = crate::dial::dial_peer(&self.conn.at, peer::DATA_PATH, limit).await.map_err(|error| {
            self.inner.node.log(format!("a data connection to {} failed: {error}", self.conn.name));
            format!("{} was not reached directly for the data connection; this machine's log says why", self.conn.name)
        })?;
        let mut socket = dialed.socket;
        let key = &self.inner.key;
        let hello = Frame::Hello {
            protocol: peer::PROTOCOL,
            version: env!("CARGO_PKG_VERSION").to_string(),
            hub: self.inner.hub.clone(),
            intent: Intent::Data { key: key.public(), proof: key.sign(Domain::Peer, &dialed.binding), stream, end },
        };
        peer::send(&mut socket, &hello).await?;
        match peer::next(&mut socket).await? {
            Frame::Welcome { name, proof } if name == self.conn.name => {
                self.conn
                    .key
                    .verify(Domain::Peer, &dialed.binding, &proof)
                    .map_err(|_| format!("what answered does not hold {}'s key", self.conn.name))?;
                Ok(socket)
            }
            Frame::Welcome { name, .. } => Err(format!("{name} answered, not {}", self.conn.name)),
            Frame::Refused { reason } => Err(reason),
            _ => Err("it answered something other than a welcome".to_string()),
        }
    }

    async fn await_other(&self, stream: u64, end: End) -> Result<Socket, String> {
        let key = (self.conn.name.clone(), stream);
        let arrived = self.inner.direct.arrived.lock().expect("never poisoned").remove(&key);
        if let Some(arrived) = arrived {
            if arrived.end == end {
                return Err("both ends of one data connection are of one kind".to_string());
            }
            return Ok(arrived.socket);
        }
        let (hand, handed) = oneshot::channel();
        self.inner.direct.awaited.lock().expect("never poisoned").insert(key.clone(), Awaited { end, hand });
        struct Unwait<'a>(&'a Inner, (MachineName, u64));
        impl Drop for Unwait<'_> {
            fn drop(&mut self) {
                self.0.direct.awaited.lock().expect("never poisoned").remove(&self.1);
            }
        }
        let _unwait = Unwait(&self.inner, key);
        tokio::select! {
            handed = handed => handed.map_err(|_| "the other end did not come".to_string()),
            reason = self.conn.ended() => Err(format!("the direct link ended before the other end came: {reason}")),
        }
    }
}

static ARRIVALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Inner {
    /// Hands a data connection `from` opened to the call here waiting for
    /// it, or keeps it for the call about to ask: as long as the World lets
    /// any peer stay silent, after which no call here wanted it.
    pub(super) fn arrived(self: &Arc<Self>, from: MachineName, stream: u64, end: End, socket: Socket) {
        let key = (from, stream);
        let awaited = self.direct.awaited.lock().expect("never poisoned").remove(&key);
        if let Some(awaited) = awaited {
            if awaited.end == end {
                self.node.log(format!("{} opened a data connection of the wrong kind", key.0));
                return;
            }
            let _ = awaited.hand.send(socket);
            return;
        }
        let number = ARRIVALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.direct.arrived.lock().expect("never poisoned").insert(key.clone(), Arrived { end, socket, number });
        let inner = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(SILENCE).await;
            let Some(inner) = inner.upgrade() else { return };
            let mut arrived = inner.direct.arrived.lock().expect("never poisoned");
            if arrived.get(&key).is_some_and(|waiting| waiting.number == number) {
                arrived.remove(&key);
                drop(arrived);
                inner.node.log(format!(
                    "a data connection {} opened was not asked for within {} s and was closed",
                    key.0,
                    SILENCE.as_secs()
                ));
            }
        });
    }
}
