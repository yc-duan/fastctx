//! A data connection kept alive and watched from both ends. Each end says
//! something at least every `HEARTBEAT`, a WebSocket ping when it has
//! nothing else to say, and takes the other for gone once nothing of the
//! other's own has come for `SILENCE`: the same rule as the control and
//! direct links, so no connection that carries a stream waits on the
//! operating system to notice a peer is gone, and none goes quiet long
//! enough for a path that drops idle connections to cut it. Pongs are not
//! the other end's own: on a connection the hub relays, the hub answers
//! pings itself whether the far end is there or not.
//!
//! An end of an older fastctx answers pings without sending any, so a
//! connection to one that stays quiet past `SILENCE` ends; whoever holds
//! the stream opens it again.

use std::sync::{Arc, Mutex};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::dial::{self, HubSocket};
use crate::wire::{HEARTBEAT, SILENCE};

/// What the other end sent, in the order it sent it.
pub(super) enum Received {
    Bytes(Vec<u8>),
    Word(String),
}

enum Out {
    Message(WsMessage, oneshot::Sender<Result<(), String>>),
    Close(oneshot::Sender<()>),
}

/// One end of a data connection, read and written by tasks of its own.
pub(super) struct Line {
    incoming: mpsc::Receiver<Received>,
    /// Why reading stopped, once it has; none when the other end closed.
    stopped: Arc<Mutex<Option<String>>>,
    outgoing: mpsc::UnboundedSender<Out>,
    reader: tokio::task::JoinHandle<()>,
    writer: tokio::task::JoinHandle<()>,
}

impl Line {
    /// Takes over `socket`; `cut`, when this machine cuts the call the
    /// connection goes with, ends it with the reason.
    pub(super) fn new(socket: HubSocket, cut: Option<watch::Receiver<Option<String>>>) -> Self {
        let (sink, stream) = socket.split();
        // One message waits for its holder at most: what follows is not
        // read until it is taken, so a slow holder slows the sender.
        let (received, incoming) = mpsc::channel(1);
        let (outgoing, sending) = mpsc::unbounded_channel();
        let stopped = Arc::new(Mutex::new(None));
        Self {
            incoming,
            stopped: stopped.clone(),
            outgoing,
            reader: tokio::spawn(read(stream, received, stopped, cut)),
            writer: tokio::spawn(write(sink, sending)),
        }
    }

    /// Sends `message`; returns once it is written.
    pub(super) async fn send(&self, message: WsMessage) -> Result<(), String> {
        let (sent, written) = oneshot::channel();
        self.outgoing.send(Out::Message(message, sent)).map_err(|_| self.gone())?;
        written.await.map_err(|_| self.gone())?
    }

    /// The next thing the other end sent; none once it closed the
    /// connection, and why when it ended otherwise.
    pub(super) async fn next(&mut self) -> Result<Option<Received>, String> {
        match self.incoming.recv().await {
            Some(received) => Ok(Some(received)),
            None => match self.stopped.lock().expect("never poisoned").clone() {
                None => Ok(None),
                Some(why) => Err(why),
            },
        }
    }

    /// Closes the connection the way a WebSocket closes, once what was sent
    /// before is written.
    pub(super) async fn close(self) {
        let (closed, done) = oneshot::channel();
        if self.outgoing.send(Out::Close(closed)).is_ok() {
            let _ = done.await;
        }
    }

    fn gone(&self) -> String {
        self.stopped.lock().expect("never poisoned").clone().unwrap_or_else(|| "the data connection ended".to_string())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

async fn read(
    mut stream: SplitStream<HubSocket>,
    received: mpsc::Sender<Received>,
    stopped: Arc<Mutex<Option<String>>>,
    mut cut: Option<watch::Receiver<Option<String>>>,
) {
    let mut heard = Instant::now();
    let why = loop {
        let next = tokio::select! {
            next = stream.next() => next,
            () = tokio::time::sleep_until(heard + SILENCE) => {
                break Some(format!("nothing came from the other end for {} s", SILENCE.as_secs()));
            }
            why = was_cut(&mut cut) => break Some(why),
        };
        let message = match next {
            None | Some(Ok(WsMessage::Close(_))) => break None,
            Some(Err(error)) => break Some(format!("receiving: {}", dial::describe(&error))),
            Some(Ok(WsMessage::Binary(bytes))) => Received::Bytes(bytes.to_vec()),
            Some(Ok(WsMessage::Text(text))) => Received::Word(text.to_string()),
            Some(Ok(WsMessage::Ping(_))) => {
                heard = Instant::now();
                continue;
            }
            Some(Ok(WsMessage::Pong(_) | WsMessage::Frame(_))) => continue,
        };
        if received.send(message).await.is_err() {
            return;
        }
        // Waiting for the holder to take it is not the other end's silence.
        heard = Instant::now();
    };
    *stopped.lock().expect("never poisoned") = why;
}

/// The reason this machine cut the call, once it does.
async fn was_cut(cut: &mut Option<watch::Receiver<Option<String>>>) -> String {
    let Some(cut) = cut else { return std::future::pending().await };
    let why = cut.wait_for(Option::is_some).await.map(|why| why.clone().unwrap_or_default());
    match why {
        Ok(why) => why,
        Err(_) => std::future::pending().await,
    }
}

async fn write(mut sink: SplitSink<HubSocket, WsMessage>, mut sending: mpsc::UnboundedReceiver<Out>) {
    let mut beat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
    beat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            out = sending.recv() => match out {
                Some(Out::Message(message, sent)) => {
                    let result = sink.send(message).await.map_err(|error| format!("sending: {}", dial::describe(&error)));
                    let failed = result.is_err();
                    let _ = sent.send(result);
                    if failed {
                        return;
                    }
                }
                Some(Out::Close(closed)) => {
                    // A close waits on the other end only as long as any
                    // silence is allowed to last.
                    let _ = tokio::time::timeout(SILENCE, sink.close()).await;
                    let _ = closed.send(());
                    return;
                }
                None => return,
            },
            _ = beat.tick() => {
                if sink.send(WsMessage::Ping(Default::default())).await.is_err() {
                    return;
                }
            }
        }
    }
}
