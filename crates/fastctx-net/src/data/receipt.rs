//! Receipt work is distinct from transport heartbeats. Only an owned receiver
//! operation may extend the wait while it is writing or committing the body.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

const WORKING: &[u8] = b"fastctx:receipt-work:1";

/// A receiver's real local work that must finish before it can send its receipt.
/// Keep it with that work, including non-cancellable disk operations.
pub struct Settlement(Arc<Progress>);

impl Drop for Settlement {
    fn drop(&mut self) {
        self.0.completed.fetch_add(1, Ordering::Relaxed);
        self.0.working.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A handle for receiver work performed inside a byte consumer or blocking task.
#[derive(Clone, Default)]
pub struct ReceiptWork(pub(super) Arc<Progress>);

impl ReceiptWork {
    pub fn begin(&self) -> Settlement {
        self.0.hold()
    }
    /// Reports active work or a completed operation not yet advertised.
    pub fn changed(&self, seen: &mut usize) -> bool {
        let completed = self.0.completed.load(Ordering::Relaxed);
        let changed = completed != *seen || self.0.working.load(Ordering::Relaxed) != 0;
        *seen = completed;
        changed
    }
}

pub(super) struct Progress {
    working: AtomicUsize,
    completed: AtomicUsize,
    heard: watch::Sender<Option<Instant>>,
}

impl Default for Progress {
    fn default() -> Self {
        Self { working: AtomicUsize::new(0), completed: AtomicUsize::new(0), heard: watch::channel(None).0 }
    }
}

impl Progress {
    pub fn hold(self: &Arc<Self>) -> Settlement {
        self.working.fetch_add(1, Ordering::Relaxed);
        Settlement(self.clone())
    }

    pub fn heard(&self, ping: &[u8]) {
        if ping == WORKING {
            self.heard.send_replace(Some(Instant::now()));
        }
    }
    pub fn subscribe(&self) -> watch::Receiver<Option<Instant>> {
        self.heard.subscribe()
    }
}

/// An explicit receiver-work notification. Older data peers ignore the payload.
/// It is separate from the ordinary byte-count ping, which must keep its shape.
pub fn settling_ping() -> Message {
    Message::Ping(WORKING.to_vec().into())
}
