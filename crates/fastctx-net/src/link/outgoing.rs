//! Control frames queued for the socket writer, written as they are queued.
//! An answer can await the actual write before releasing the node's handoff
//! guard. At most `QUEUED` frames or `QUEUED_BYTES` bytes wait: past that
//! the hub takes nothing, and the session ends (`crate::queue`).
//!
//! Heartbeats and receipts for letters go a way of their own, written
//! before anything else that waits: a backlog of letters and answers never
//! holds back the frame that says this machine is there, nor the one that
//! lets the hub send the next window (`specs/world-engine.md`, 连接与判活).

use tokio::sync::{mpsc, oneshot};

use crate::queue;
use crate::wire::Up;

/// The most frames that wait for the writer. Letters go a window at a time
/// and the rest answers or tells something, so a hub that reads never
/// leaves this many.
const QUEUED: usize = 8192;
/// The most bytes of them.
const QUEUED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Outgoing {
    queued: queue::Sender<Frame>,
    /// Heartbeats and receipts: one every few seconds, and one a window of
    /// letters, so this way needs no bound.
    first: mpsc::UnboundedSender<Frame>,
}

pub(crate) struct Frame {
    /// The frame as it is written.
    pub bytes: Vec<u8>,
    pub written: Option<oneshot::Sender<()>>,
}

/// The writer's end: the first way first, then the rest in the order queued.
pub(crate) struct Writing {
    queued: queue::Receiver<Frame>,
    first: mpsc::UnboundedReceiver<Frame>,
}

impl Writing {
    pub async fn recv(&mut self) -> Option<Frame> {
        tokio::select! {
            biased;
            Some(frame) = self.first.recv() => Some(frame),
            frame = self.queued.recv() => frame,
        }
    }
}

impl Outgoing {
    pub fn channel() -> (Self, Writing) {
        let (queued, receiving) = queue::bounded(QUEUED, QUEUED_BYTES, "link.outgoing");
        let (first, first_receiving) = mpsc::unbounded_channel();
        (Self { queued, first }, Writing { queued: receiving, first: first_receiving })
    }

    fn queue(&self, value: &Up, written: Option<oneshot::Sender<()>>) -> Result<(), ()> {
        let bytes = crate::wire::encode(value);
        let size = bytes.len();
        self.queued.send(Frame { bytes, written }, size).map_err(|_| ())
    }

    pub fn send(&self, value: Up) -> Result<(), ()> {
        self.queue(&value, None)
    }

    /// Sends a heartbeat ahead of everything queued.
    pub fn send_first(&self, value: Up) -> Result<(), ()> {
        self.first.send(Frame { bytes: crate::wire::encode(&value), written: None }).map_err(|_| ())
    }

    /// Sends a receipt ahead of everything queued and waits until it is
    /// written.
    pub async fn write_first(&self, value: Up) -> Result<(), String> {
        let (written, received) = oneshot::channel();
        self.first
            .send(Frame { bytes: crate::wire::encode(&value), written: Some(written) })
            .map_err(|_| "the control connection closed before the receipt was queued".to_string())?;
        received.await.map_err(|_| "the control connection closed before the receipt was written".to_string())
    }

    pub async fn write(&self, value: Up) -> Result<(), String> {
        let (written, received) = oneshot::channel();
        self.queue(&value, Some(written))
            .map_err(|()| "the control connection closed before the answer was queued".to_string())?;
        received.await.map_err(|_| "the control connection closed before the answer was written".to_string())
    }

    /// Resolves once a frame was refused because the hub took nothing.
    pub async fn overflowed(&self) {
        self.queued.overflowed().await;
    }
}
