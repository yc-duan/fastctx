//! Control frames queued for the socket writer. An answer can await the
//! actual write before releasing the node's handoff guard.

use tokio::sync::{mpsc, oneshot};

use crate::wire::Up;

#[derive(Clone)]
pub(crate) struct Outgoing(mpsc::UnboundedSender<Frame>);

pub(crate) struct Frame {
    pub value: Up,
    pub written: Option<oneshot::Sender<()>>,
}

impl Outgoing {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<Frame>) {
        let (sender, receiver) = mpsc::unbounded_channel();
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
}
