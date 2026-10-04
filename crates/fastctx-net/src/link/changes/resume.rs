//! logind's resume signal is immediate, including sleeps shorter than a
//! heartbeat. The BOOTTIME timer also serves machines without logind.

use futures_util::StreamExt as _;
use tokio::sync::mpsc;

pub(super) struct Resume {
    receiver: mpsc::UnboundedReceiver<Result<(), String>>,
    task: tokio::task::JoinHandle<()>,
}

impl Resume {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut reported = None;
            loop {
                match listen(&sender).await {
                    Ok(()) => return,
                    Err(error) => {
                        let error = error.to_string();
                        if reported.as_ref() != Some(&error) {
                            let message = format!(
                                "logind resume notifications unavailable: {error}; checking again and monitoring running-time clocks every {} s",
                                crate::wire::HEARTBEAT.as_secs()
                            );
                            if sender.send(Err(message)).is_err() {
                                return;
                            }
                            reported = Some(error);
                        }
                    }
                }
                // logind can start after the node at boot or restart. Use
                // the existing liveness cadence to discover its return.
                tokio::time::sleep(crate::wire::HEARTBEAT).await;
            }
        });
        Self { receiver, task }
    }

    pub async fn recv(&mut self) -> Result<(), String> {
        match self.receiver.recv().await {
            Some(event) => event,
            None => std::future::pending().await,
        }
    }
}

impl Drop for Resume {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn listen(sender: &mpsc::UnboundedSender<Result<(), String>>) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let mut signals = proxy.receive_signal("PrepareForSleep").await?;
    while let Some(signal) = signals.next().await {
        let sleeping: bool = signal.body().deserialize()?;
        if !sleeping && sender.send(Ok(())).is_err() {
            return Ok(());
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the system bus signal stream ended").into())
}
