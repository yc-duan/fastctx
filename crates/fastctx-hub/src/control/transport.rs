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

// Both count and bytes are bounded. A full legitimate letter window
// fits; a peer ignoring the window cannot allocate unbounded business work.
const INPUT_FRAMES: usize = 2048;
const INPUT_BYTES: usize = 16 * 1024 * 1024;
const OUTPUT_FRAMES: usize = 4;
/// How long a member's frame waits for room among what every member has
/// waiting before that member is let go; it reads no further meanwhile.
const SHARED_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// What every member together may have waiting to be handled: a sixteenth
/// of the machine's memory, from 64 MiB to 512 MiB, so ten thousand members
/// each sending up to its own budget cannot take the machine's memory
/// (`specs/world-foundation.md`, 宪法第三条).
struct Shared {
    room: Arc<tokio::sync::Semaphore>,
    bytes: usize,
    /// How much of it was used, told in the hub's health.
    used: &'static fastctx_platform::health::Bound,
}

fn shared() -> &'static Shared {
    static SHARED: std::sync::OnceLock<Shared> = std::sync::OnceLock::new();
    SHARED.get_or_init(|| {
        let physical = fastctx_platform::memory::physical().unwrap_or(1 << 30);
        let bytes = usize::try_from((physical / 16).clamp(64 << 20, 512 << 20)).unwrap_or(usize::MAX);
        Shared {
            room: Arc::new(tokio::sync::Semaphore::new(bytes)),
            bytes,
            used: fastctx_platform::health::bound("hub.input"),
        }
    })
}

pub(super) struct Input {
    pub bytes: bytes::Bytes,
    _budget: tokio::sync::OwnedSemaphorePermit,
    _shared: tokio::sync::OwnedSemaphorePermit,
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
    pub ended: watch::Receiver<Option<Ended>>,
    task: tokio::task::JoinHandle<()>,
}

/// Why a member's transport ended.
#[derive(Clone, Debug)]
pub(super) struct Ended {
    pub reason: String,
    /// Nothing was heard from the member for `SILENCE`.
    pub silent: bool,
}

impl Ended {
    fn of(reason: impl Into<String>) -> Self {
        Self { reason: reason.into(), silent: false }
    }
}

impl Transport {
    pub(super) fn start(socket: crate::Socket, runtime: &tokio::runtime::Handle) -> Self {
        let (input, incoming) = mpsc::channel(INPUT_FRAMES);
        let (outgoing, output) = mpsc::channel(OUTPUT_FRAMES);
        let (end, ended) = watch::channel(None);
        let (beats, heartbeat) = watch::channel(None);
        let task = fastctx_platform::task::spawn_on(runtime, async move {
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
) -> Ended {
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
        ended = reader => ended,
        reason = writer => Ended::of(reason),
    }
}

async fn read(
    mut stream: futures_util::stream::SplitStream<crate::Socket>,
    pulse: Arc<Pulse>,
    input: mpsc::Sender<Input>,
    pongs: watch::Sender<Option<u64>>,
    beats: watch::Sender<Option<Up>>,
) -> Ended {
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
                return Ended { reason: format!("nothing was heard from it for {} s", SILENCE.as_secs()), silent: true };
            }
        };
        heard = tokio::time::Instant::now();
        let bytes = match next {
            None | Some(Ok(Message::Close(_))) => return Ended::of("peer closed"),
            Some(Err(error)) => return Ended::of(fastctx_net::dial::describe(&error)),
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
            return Ended::of("incoming control frame exceeds its byte limit");
        }
        // A heartbeat has bounded metadata and replaces its predecessor.
        // Preserve an unconsumed path change across later heartbeats that
        // omit paths; counters only move forward on this session.
        if let Ok(Heartbeat { kind, n, took, paths, held, health, workbench }) =
            serde_json::from_slice::<Heartbeat>(&bytes)
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
                *previous = Some(Up::Ping { n, took, paths: paths.or(prior_paths), held, health, workbench });
            });
            continue;
        }
        let size = u32::try_from(bytes.len().max(1)).unwrap_or(u32::MAX);
        let Ok(permit) = budget.clone().try_acquire_many_owned(size) else {
            return Ended::of("member exceeded its pending input byte budget");
        };
        let all = shared();
        let Ok(Ok(room)) = tokio::time::timeout(SHARED_WAIT, all.room.clone().acquire_many_owned(size)).await else {
            return Ended::of("what every member has waiting at the hub stayed full; letting this one go");
        };
        all.used.note(all.bytes.saturating_sub(all.room.available_permits()), all.bytes);
        if input.try_send(Input { bytes, _budget: permit, _shared: room }).is_err() {
            return Ended::of("member exceeded its pending input frame budget");
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
    #[serde(default)]
    workbench: Option<bool>,
}
