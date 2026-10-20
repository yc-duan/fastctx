//! A data connection kept alive and watched from both ends. Each end says
//! something at least every `HEARTBEAT`, a WebSocket ping when it has
//! nothing else to say, and takes the other for gone once nothing of the
//! other's own has come for `SILENCE`: the same rule as the control and
//! direct links, so no connection that carries a stream waits on the
//! operating system to notice a peer is gone, and none goes quiet long
//! enough for a path that drops idle connections to cut it. Pongs are not
//! the other end's own: on a connection the hub relays, the hub answers
//! pings itself whether the far end is there or not; so neither they nor
//! the few bytes they take count, and a message on its way counts once
//! more of it came within a silence than such answers could make.
//!
//! The two directions end together: once reading stops, for silence, a
//! break or a cut, writing stops too and whatever waits to be written is
//! told why. Each ping says how much this end took of what the other sent,
//! and nothing written goes on once neither its bytes went nor the other
//! end said it took more for the silence (`crate::pulse`): an end that does
//! not take what it is sent is as gone as one that says nothing, however
//! lively its own pings, while a slow line still carrying it is not.
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

/// How much of a message on its way has to come within a silence to show
/// the other end is there: more than the hub's answers to this end's own
/// pings, a few dozen bytes each, could make.
const MOVING: u64 = 2048;

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
    /// Set once the connection is over, by whichever direction ended it.
    over: watch::Receiver<bool>,
    outgoing: mpsc::UnboundedSender<Out>,
    reader: tokio::task::JoinHandle<()>,
    writer: tokio::task::JoinHandle<()>,
    pub(super) receipt: Arc<super::receipt::Progress>,
}

impl Line {
    /// Takes over `socket`; `cut`, when this machine cuts the call the
    /// connection goes with, ends it with the reason.
    pub(super) fn new(socket: HubSocket, cut: Option<watch::Receiver<Option<String>>>) -> Self {
        let pulse = crate::transport::pulse_of(&socket);
        let (sink, stream) = socket.split();
        // One message waits for its holder at most: what follows is not
        // read until it is taken, so a slow holder slows the sender.
        let (received, incoming) = mpsc::channel(1);
        let (outgoing, sending) = mpsc::unbounded_channel();
        let stopped = Arc::new(Mutex::new(None));
        let (ending, over) = watch::channel(false);
        let ending = Arc::new(ending);
        let receipt = Arc::new(super::receipt::Progress::default());
        Self {
            incoming,
            stopped: stopped.clone(),
            over: over.clone(),
            outgoing,
            reader: tokio::spawn(read(
                stream,
                received,
                stopped.clone(),
                cut,
                ending.clone(),
                pulse.clone(),
                receipt.clone(),
            )),
            writer: tokio::spawn(write(sink, sending, over, stopped, ending, pulse, receipt.clone())),
            receipt,
        }
    }

    pub(super) fn sender(&self) -> Sender {
        Sender { outgoing: self.outgoing.clone(), over: self.over.clone(), stopped: self.stopped.clone() }
    }

    pub(super) async fn send(&self, message: WsMessage) -> Result<(), String> {
        self.sender().send(message).await
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
}

pub(super) struct Sender {
    outgoing: mpsc::UnboundedSender<Out>,
    over: watch::Receiver<bool>,
    stopped: Arc<Mutex<Option<String>>>,
}

impl Sender {
    /// Sends `message`; returns once it is written, or once the connection
    /// is over, whichever comes first.
    pub(super) async fn send(&self, message: WsMessage) -> Result<(), String> {
        let (sent, written) = oneshot::channel();
        self.outgoing.send(Out::Message(message, sent)).map_err(|_| self.gone())?;
        let mut over = self.over.clone();
        tokio::select! {
            written = written => written.map_err(|_| self.gone())?,
            _ = over.wait_for(|over| *over) => Err(self.gone()),
        }
    }

    fn gone(&self) -> String {
        self.stopped.lock().expect("never poisoned").clone().unwrap_or_else(|| "the data connection ended".into())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

/// Notes why the connection ended, unless a reason is noted already, and
/// tells both directions it is over.
fn end(stopped: &Mutex<Option<String>>, ending: &watch::Sender<bool>, why: Option<String>) {
    {
        let mut stopped = stopped.lock().expect("never poisoned");
        if stopped.is_none() {
            *stopped = why;
        }
    }
    ending.send_replace(true);
}

async fn read(
    mut stream: SplitStream<HubSocket>,
    received: mpsc::Sender<Received>,
    stopped: Arc<Mutex<Option<String>>>,
    mut cut: Option<watch::Receiver<Option<String>>>,
    ending: Arc<watch::Sender<bool>>,
    pulse: Arc<crate::pulse::Pulse>,
    receipt: Arc<super::receipt::Progress>,
) {
    let mut over = ending.subscribe();
    let mut heard = Instant::now();
    let mut taken = pulse.taken();
    let why = loop {
        let next = tokio::select! {
            next = stream.next() => next,
            () = tokio::time::sleep_until(heard + SILENCE) => {
                // A message still coming in shows the other end is there.
                if pulse.taken() >= taken + MOVING {
                    heard = Instant::now();
                    taken = pulse.taken();
                    continue;
                }
                break Some(format!("nothing came from the other end for {} s", SILENCE.as_secs()));
            }
            why = was_cut(&mut cut) => break Some(why),
            // Writing ended it: it says why itself.
            _ = over.wait_for(|over| *over) => break None,
        };
        let message = match next {
            None | Some(Ok(WsMessage::Close(_))) => break None,
            Some(Err(error)) => break Some(format!("receiving: {}", dial::describe(&error))),
            Some(Ok(WsMessage::Binary(bytes))) => Received::Bytes(bytes.to_vec()),
            Some(Ok(WsMessage::Text(text))) => Received::Word(text.to_string()),
            Some(Ok(WsMessage::Ping(said))) => {
                receipt.heard(&said);
                if let Some(took) = crate::pulse::said(&said) {
                    pulse.told(took);
                }
                heard = Instant::now();
                taken = pulse.taken();
                continue;
            }
            Some(Ok(WsMessage::Pong(_) | WsMessage::Frame(_))) => continue,
        };
        // Waiting for the holder to take it is not the other end's silence,
        // but a cut or the end of writing still ends the connection.
        tokio::select! {
            sent = received.send(message) => if sent.is_err() { return },
            why = was_cut(&mut cut) => break Some(why),
            _ = over.wait_for(|over| *over) => break None,
        }
        heard = Instant::now();
        taken = pulse.taken();
    };
    end(&stopped, &ending, why);
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

/// Writes `message`, for as long as its bytes keep going out.
async fn put(
    sink: &mut SplitSink<HubSocket, WsMessage>,
    pulse: &crate::pulse::Pulse,
    message: WsMessage,
) -> Result<(), String> {
    match pulse.writing(sink.send(message)).await {
        Ok(sent) => sent.map_err(|error| format!("sending: {}", dial::describe(&error))),
        Err(quiet) => Err(quiet.to_string()),
    }
}

async fn write(
    mut sink: SplitSink<HubSocket, WsMessage>,
    mut sending: mpsc::UnboundedReceiver<Out>,
    mut over: watch::Receiver<bool>,
    stopped: Arc<Mutex<Option<String>>>,
    ending: Arc<watch::Sender<bool>>,
    pulse: Arc<crate::pulse::Pulse>,
    receipt: Arc<super::receipt::Progress>,
) {
    let work = super::receipt::ReceiptWork(receipt);
    let mut completed = 0;
    let mut beat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
    beat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        // A write under way is given up once the connection is over.
        let next = tokio::select! {
            out = sending.recv() => Some(out),
            _ = beat.tick() => None,
            _ = over.wait_for(|over| *over) => return,
        };
        let Some(next) = next else {
            let beating = async {
                put(&mut sink, &pulse, crate::pulse::ping(&pulse)).await?;
                if work.changed(&mut completed) {
                    put(&mut sink, &pulse, super::receipt::settling_ping()).await?;
                }
                Ok::<(), String>(())
            };
            let result = tokio::select! {
                result = beating => result,
                _ = over.wait_for(|over| *over) => return,
            };
            if let Err(why) = result {
                end(&stopped, &ending, Some(why));
                return;
            }
            continue;
        };
        match next {
            Some(Out::Message(message, sent)) => {
                let writing = put(&mut sink, &pulse, message);
                let result = tokio::select! {
                    result = writing => result,
                    _ = over.wait_for(|over| *over) => return,
                };
                let failed = result.as_ref().err().cloned();
                let _ = sent.send(result);
                if let Some(why) = failed {
                    end(&stopped, &ending, Some(why));
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
        }
    }
}
