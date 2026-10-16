//! A queue for a connection's writer that holds at most a set number of
//! frames: a connection whose other end takes nothing would otherwise grow
//! one without end (`world-concurrency.md`, 机器之间). Past the bound a
//! frame is refused and the connection's owner is told, which ends the
//! connection: its writer is stuck, and a new one starts afresh.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{Notify, mpsc};

/// The sending half; clones share the bound.
pub(crate) struct Sender<T> {
    sender: mpsc::UnboundedSender<T>,
    shared: Arc<Shared>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone(), shared: self.shared.clone() }
    }
}

/// The receiving half, the writer's.
pub(crate) struct Receiver<T> {
    receiver: mpsc::UnboundedReceiver<T>,
    shared: Arc<Shared>,
}

struct Shared {
    queued: AtomicUsize,
    bound: usize,
    overflowed: Notify,
}

/// A queue holding at most `bound` frames.
pub(crate) fn bounded<T>(bound: usize) -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared { queued: AtomicUsize::new(0), bound, overflowed: Notify::new() });
    (Sender { sender, shared: shared.clone() }, Receiver { receiver, shared })
}

impl<T> Sender<T> {
    /// Queues `value`; an error when the writer is gone or `bound` frames
    /// wait already, which also wakes `overflowed`.
    pub fn send(&self, value: T) -> Result<(), T> {
        if self.shared.queued.fetch_add(1, Ordering::AcqRel) >= self.shared.bound {
            self.shared.queued.fetch_sub(1, Ordering::AcqRel);
            self.shared.overflowed.notify_one();
            return Err(value);
        }
        self.sender.send(value).map_err(|refused| {
            self.shared.queued.fetch_sub(1, Ordering::AcqRel);
            refused.0
        })
    }

    /// Resolves once a frame was refused for want of room.
    pub async fn overflowed(&self) {
        self.shared.overflowed.notified().await;
    }
}

impl<T> Receiver<T> {
    pub async fn recv(&mut self) -> Option<T> {
        let value = self.receiver.recv().await;
        if value.is_some() {
            self.shared.queued.fetch_sub(1, Ordering::AcqRel);
        }
        value
    }
}
