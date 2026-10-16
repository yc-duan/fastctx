//! A queue for a connection's writer that holds at most a set number of
//! frames and bytes: a connection whose other end takes nothing would
//! otherwise grow one without end (`world-concurrency.md`, 机器之间). Past
//! either bound a frame is refused and the connection's owner is told,
//! which ends the connection: its writer is stuck, and a new one starts
//! afresh. One frame larger than the byte bound still goes when nothing
//! else waits.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{Notify, mpsc};

/// The sending half; clones share the bounds.
pub(crate) struct Sender<T> {
    sender: mpsc::UnboundedSender<(T, usize)>,
    shared: Arc<Shared>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone(), shared: self.shared.clone() }
    }
}

/// The receiving half, the writer's.
pub(crate) struct Receiver<T> {
    receiver: mpsc::UnboundedReceiver<(T, usize)>,
    shared: Arc<Shared>,
}

struct Shared {
    queued: AtomicUsize,
    bytes: AtomicUsize,
    bound: usize,
    bound_bytes: usize,
    overflowed: Notify,
}

/// A queue holding at most `bound` frames and `bound_bytes` bytes.
pub(crate) fn bounded<T>(bound: usize, bound_bytes: usize) -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        queued: AtomicUsize::new(0),
        bytes: AtomicUsize::new(0),
        bound,
        bound_bytes,
        overflowed: Notify::new(),
    });
    (Sender { sender, shared: shared.clone() }, Receiver { receiver, shared })
}

impl<T> Sender<T> {
    /// Queues `value`, `size` bytes of it; an error when the writer is gone
    /// or the bounds are reached, which also wakes `overflowed`.
    pub fn send(&self, value: T, size: usize) -> Result<(), T> {
        let before = self.shared.queued.fetch_add(1, Ordering::AcqRel);
        let bytes = self.shared.bytes.fetch_add(size, Ordering::AcqRel);
        if before >= self.shared.bound || (before > 0 && bytes + size > self.shared.bound_bytes) {
            self.shared.queued.fetch_sub(1, Ordering::AcqRel);
            self.shared.bytes.fetch_sub(size, Ordering::AcqRel);
            self.shared.overflowed.notify_one();
            return Err(value);
        }
        self.sender.send((value, size)).map_err(|refused| {
            self.shared.queued.fetch_sub(1, Ordering::AcqRel);
            self.shared.bytes.fetch_sub(size, Ordering::AcqRel);
            refused.0.0
        })
    }

    /// Resolves once a frame was refused for want of room.
    pub async fn overflowed(&self) {
        self.shared.overflowed.notified().await;
    }
}

impl<T> Receiver<T> {
    pub async fn recv(&mut self) -> Option<T> {
        let (value, size) = self.receiver.recv().await?;
        self.shared.queued.fetch_sub(1, Ordering::AcqRel);
        self.shared.bytes.fetch_sub(size, Ordering::AcqRel);
        Some(value)
    }
}
