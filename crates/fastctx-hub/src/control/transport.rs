//! A member's transport, isolated from all hub state and disk work.
//! Heartbeats bypass business queues in both directions. Exceeding a
//! connection's input budget closes only that connection, without waiting
//! for its business consumer to make room.

use std::sync::Arc;

use fastctx_net::pulse::Pulse;
use fastctx_net::wire::{self, Down, HEARTBEAT, SILENCE, Up};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

// 2026-10-31: bound both count and bytes. A full legitimate letter window
// fits; a peer ignoring the window cannot allocate unbounded business work.
const INPUT_FRAMES: usize = 2048;
const INPUT_BYTES: usize = 16 * 1024 * 1024;
const OUTPUT_FRAMES: usize = 4;

pub(super) struct Input {
    pub bytes: bytes::Bytes,
    _budget: tokio::sync::OwnedSemaphorePermit,
}

pub(super) struct Packet {
    pub bytes: Vec<u8>,
    pub last: bool,
    pub written: Option<tokio::sync::oneshot::Sender<()>>,
}

pub(super) struct Transport {
    pub incoming: mpsc::Receiver<Input>,
    pub outgoing: mpsc::Sender<Packet>,
    pub heartbeat: watch::Receiver<Option<Up>>,
    pub ended: watch::Receiver<Option<String>>,
    task: tokio::task::JoinHandle<()>,
}

impl Transport {
    pub(super) fn start(socket: crate::Socket, runtime: &tokio::runtime::Handle) -> Self {
        let (input, incoming) = mpsc::channel(INPUT_FRAMES);
        let (outgoing, output) = mpsc::channel(OUTPUT_FRAMES);
        let (end, ended) = watch::channel(None);
        let (beats, heartbeat) = watch::channel(None);
        let task = runtime.spawn(async move {
            let reason = run(socket, input, output, beats).await;
            end.send_replace(Some(reason));
        });
        Self { incoming, outgoing, ended, heartbeat, task }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(
    socket: crate::Socket,
    input: mpsc::Sender<Input>,
    mut output: mpsc::Receiver<Packet>,
    beats: watch::Sender<Option<Up>>,
) -> String {
    let pulse = crate::pulse_of(&socket);
    let (mut sink, stream) = socket.split();
    let (pongs, mut pong) = watch::channel(None);
    let reader = read(stream, pulse.clone(), input, pongs, beats);
    let writer = async {
        let mut welcomed = false;
        loop {
            let (message, last, receipt) = tokio::select! {
                biased;
                changed = pong.changed(), if welcomed => {
                    if changed.is_err() { return "reader stopped".into(); }
                    let Some(n) = *pong.borrow_and_update() else { continue };
                    let frame = Down::Pong {
                        n, at: Some(jiff::Timestamp::now()), took: Some(pulse.taken()), afoot_ready: false,
                    };
                    (Message::Binary(wire::encode(&frame).into()), false, None)
                }
                next = output.recv() => match next {
                    Some(packet) => { welcomed = true; (Message::Binary(packet.bytes.into()), packet.last, packet.written) },
                    None => return "business writer stopped".into(),
                },
                () = tokio::time::sleep(HEARTBEAT) => (fastctx_net::pulse::ping(&pulse), false, None),
            };
            match pulse.writing(sink.send(message)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return fastctx_net::dial::describe(&error),
                Err(error) => return error.to_string(),
            }
            if let Some(receipt) = receipt {
                let _ = receipt.send(());
            }
            if last {
                let _ = tokio::time::timeout(SILENCE, sink.close()).await;
                return "session ended".into();
            }
        }
    };
    tokio::select! {
        reason = reader => reason,
        reason = writer => reason,
    }
}

async fn read(
    mut stream: futures_util::stream::SplitStream<crate::Socket>,
    pulse: Arc<Pulse>,
    input: mpsc::Sender<Input>,
    pongs: watch::Sender<Option<u64>>,
    beats: watch::Sender<Option<Up>>,
) -> String {
    let budget = Arc::new(tokio::sync::Semaphore::new(INPUT_BYTES));
    let mut heard = tokio::time::Instant::now();
    loop {
        let next = tokio::select! {
            next = stream.next() => next,
            () = tokio::time::sleep_until(heard + SILENCE) => {
                if let Some(at) = pulse.since(heard) {
                    heard = at;
                    continue;
                }
                return format!("nothing was heard from it for {} s", SILENCE.as_secs());
            }
        };
        heard = tokio::time::Instant::now();
        let bytes = match next {
            None | Some(Ok(Message::Close(_))) => return "peer closed".into(),
            Some(Err(error)) => return fastctx_net::dial::describe(&error),
            Some(Ok(Message::Binary(bytes))) => bytes,
            Some(Ok(Message::Ping(bytes) | Message::Pong(bytes))) => {
                if let Some(took) = fastctx_net::pulse::said(&bytes) {
                    pulse.told(took);
                }
                continue;
            }
            Some(Ok(_)) => continue,
        };
        if bytes.len() > wire::MAX_CONTROL_FRAME {
            return "incoming control frame exceeds its byte limit".into();
        }
        // A heartbeat has bounded metadata and replaces its predecessor.
        // Preserve an unconsumed path change across later heartbeats that
        // omit paths; counters only move forward on this session.
        if let Ok(Heartbeat { kind, n, took, paths, held, health }) = serde_json::from_slice::<Heartbeat>(&bytes)
            && kind == "ping"
        {
            if let Some(took) = took {
                pulse.told(took);
            }
            pongs.send_replace(Some(n));
            beats.send_modify(|previous| {
                let prior_paths = match previous.take() {
                    Some(Up::Ping { paths, .. }) => paths,
                    _ => None,
                };
                *previous = Some(Up::Ping { n, took, paths: paths.or(prior_paths), held, health });
            });
            continue;
        }
        let Ok(permit) = budget.clone().try_acquire_many_owned(bytes.len().max(1) as u32) else {
            return "member exceeded its pending input byte budget".into();
        };
        if input.try_send(Input { bytes, _budget: permit }).is_err() {
            return "member exceeded its pending input frame budget".into();
        }
    }
}

#[derive(serde::Deserialize)]
struct Heartbeat {
    #[serde(rename = "f")]
    kind: String,
    #[serde(default)]
    n: u64,
    #[serde(default)]
    took: Option<u64>,
    #[serde(default)]
    paths: Option<fastctx_net::directory::Paths>,
    #[serde(default)]
    held: Option<u64>,
    #[serde(default)]
    health: Option<fastctx_types::health::Health>,
}
