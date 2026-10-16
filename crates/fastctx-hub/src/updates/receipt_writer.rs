//! The receiving hub reports work independently of its file publication task.

use crate::{Socket, WsMessage};
use fastctx_net::{
    data::ReceiptWork,
    pulse::Pulse,
    wire::{HEARTBEAT, SILENCE},
};
use futures_util::{SinkExt as _, stream::SplitSink};
use std::sync::Arc;
use tokio::sync::oneshot;

pub(super) struct Writer {
    done: Option<oneshot::Sender<String>>,
    task: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

impl Writer {
    pub fn new(mut sink: SplitSink<Socket, WsMessage>, pulse: Arc<Pulse>, work: ReceiptWork) -> Self {
        let (done, mut finish) = oneshot::channel::<String>();
        let task = tokio::spawn(async move {
            let mut completed = 0;
            let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
            beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    word = &mut finish => {
                        let word = word.map_err(|_| "the package receipt owner ended".to_string())?;
                        send(&mut sink, &pulse, WsMessage::Text(word.into())).await?;
                        let _ = tokio::time::timeout(SILENCE, sink.close()).await;
                        return Ok(());
                    }
                    _ = beat.tick() => {
                        send(&mut sink, &pulse, fastctx_net::pulse::ping(&pulse)).await?;
                        if work.changed(&mut completed) {
                            send(&mut sink, &pulse, fastctx_net::data::settling_ping()).await?;
                        }
                    }
                }
            }
        });
        Self { done: Some(done), task: Some(task) }
    }

    pub async fn finish(mut self, word: String) -> Result<(), String> {
        self.done.take().expect("one receipt").send(word).map_err(|_| "the receipt writer ended".to_string())?;
        let result = self.task.as_mut().expect("one writer").await.map_err(|error| error.to_string())?;
        self.task.take();
        result
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn send(sink: &mut SplitSink<Socket, WsMessage>, pulse: &Pulse, message: WsMessage) -> Result<(), String> {
    pulse.writing(sink.send(message)).await.map_err(|error| error.to_string())?.map_err(|error| error.to_string())
}
