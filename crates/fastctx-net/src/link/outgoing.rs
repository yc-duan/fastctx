//! Control frames queued for the socket writer. An answer can await the
//! actual write before releasing the node's handoff guard. At most
//! `QUEUED` frames wait: past that the hub takes nothing, and the session
//! ends (`crate::queue`).

use tokio::sync::oneshot;

use crate::queue;
use crate::wire::Up;

/// The most frames that wait for the writer. Letters go a window at a time
/// and the rest answers or tells something, so a hub that reads never
/// leaves this many.
const QUEUED: usize = 8192;

#[derive(Clone)]
pub(crate) struct Outgoing(queue::Sender<Frame>);

pub(crate) struct Frame {
    pub value: Up,
    pub written: Option<oneshot::Sender<()>>,
}

impl Outgoing {
    pub fn channel() -> (Self, queue::Receiver<Frame>) {
        let (sender, receiver) = queue::bounded(QUEUED);
        (Self(sender), receiver)
    }

    pub fn send(&self, value: Up) -> Result<(), ()> {
        self.0.send(Frame { value, written: None }).map_err(|_| ())
    }

    pub async fn write(&self, value: Up) -> Result<(), String> {
        let (written, received) = oneshot::channel();
        self.0
            .send(Frame { value, written: Some(written) })
            .map_err(|_| "the control connection closed before the answer was queued".to_string())?;
        received.await.map_err(|_| "the control connection closed before the answer was written".to_string())
    }

    /// Resolves once a frame was refused because the hub took nothing.
    pub async fn overflowed(&self) {
        self.0.overflowed().await;
    }
}
