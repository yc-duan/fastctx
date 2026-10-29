use super::Wake;
use fastctx_net::{reassembly::Reservation, watch_queue};
use serde::Serialize;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::ipc::Channel;

const CHUNK: usize = 64 * 1024;
const MAX_PACKET: usize = 64 * 1024 * 1024;

struct Encoded {
    text: String,
    key: Option<String>,
    _memory: Reservation,
}
struct Active {
    encoded: Encoded,
    offset: usize,
}

/// Numbered replies are replayable until acknowledged, including a gap. Only one
/// chunk can be in flight per page stream; partial packets are never shown.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Read {
    Chunk { number: u64, offset: usize, total: usize, text: String },
    Gap { number: u64 },
    Idle,
    Closed,
}
impl Read {
    fn number(&self) -> Option<u64> {
        match self {
            Self::Chunk { number, .. } | Self::Gap { number } => Some(*number),
            _ => None,
        }
    }
}
struct State {
    sender: Option<watch_queue::Sender<Encoded>>,
    receiver: watch_queue::Receiver<Encoded>,
    generation: u64,
    next: u64,
    last: Option<Read>,
    active: Option<Active>,
    notified: bool,
    closed: bool,
    finished: bool,
}
pub(crate) struct Mailbox {
    id: u64,
    wake: Channel<Wake>,
    state: Mutex<State>,
    restart: tokio::sync::watch::Sender<u64>,
}
fn queue() -> (watch_queue::Sender<Encoded>, watch_queue::Receiver<Encoded>) {
    watch_queue::channel(|value: &Encoded| (value.text.len(), value.key.clone()))
}
impl Mailbox {
    pub fn new(id: u64, wake: Channel<Wake>) -> Arc<Self> {
        let (sender, receiver) = queue();
        Arc::new(Self {
            id,
            wake,
            state: Mutex::new(State {
                sender: Some(sender),
                receiver,
                generation: 0,
                next: 0,
                last: None,
                active: None,
                notified: false,
                closed: false,
                finished: false,
            }),
            restart: tokio::sync::watch::channel(0).0,
        })
    }
    pub fn restarts(&self) -> tokio::sync::watch::Receiver<u64> {
        self.restart.subscribe()
    }
    pub fn closed(&self) -> bool {
        self.state.lock().expect("WebView mailbox").closed
    }
    pub fn close(&self) {
        let mut state = self.state.lock().expect("WebView mailbox");
        self.close_state(&mut state);
    }
    fn close_state(&self, state: &mut State) {
        state.closed = true;
        state.sender.take();
        state.active = None;
        state.last = None;
        // Drop all encoded storage, including a queue a source still refers to.
        let (_, empty) = queue();
        state.receiver = empty;
        self.restart.send_replace(state.generation);
    }
    pub fn sender<T>(self: &Arc<Self>, generation: u64, key: fn(&T) -> Option<String>) -> Option<Sender<T>> {
        let state = self.state.lock().expect("WebView mailbox");
        if state.closed || state.generation != generation {
            return None;
        }
        Some(Sender {
            mailbox: self.clone(),
            generation,
            sender: state.sender.as_ref()?.clone(),
            key,
            failed: Arc::new(AtomicBool::new(false)),
        })
    }
    fn notify(&self, state: &mut State) -> Result<(), ()> {
        if state.closed {
            return Err(());
        }
        if !state.notified {
            state.notified = true;
            if self.wake.send(Wake { id: self.id }).is_err() {
                self.close_state(state);
                return Err(());
            }
        }
        Ok(())
    }
    pub fn finish(&self, generation: u64) {
        let mut state = self.state.lock().expect("WebView mailbox");
        if state.generation == generation {
            state.finished = true;
            state.sender.take();
            let _ = self.notify(&mut state);
        }
    }
    /// The next part for the page, once it has `through`; `resync`, where it
    /// could not read the part it has, makes the next a gap, so the source
    /// starts again from what is now.
    pub fn read(&self, through: u64, resync: bool) -> Result<Read, String> {
        let mut state = self.state.lock().expect("WebView mailbox");
        if state.closed {
            return Ok(Read::Closed);
        }
        if through > state.next {
            return Err("the page acknowledged an observation chunk not yet sent".into());
        }
        if let Some(last) = &state.last {
            let number = last.number().expect("numbered reply");
            if through < number {
                return Ok(last.clone());
            }
            let gap = matches!(last, Read::Gap { .. });
            state.last = None;
            if gap {
                let (sender, receiver) = queue();
                state.sender = Some(sender);
                state.receiver = receiver;
                state.active = None;
                state.finished = false;
                state.generation += 1;
                self.restart.send_replace(state.generation);
            }
        } else if through != state.next {
            return Err("the page's observation cursor is no longer held".into());
        }
        if resync {
            state.active = None;
            state.next += 1;
            let gap = Read::Gap { number: state.next };
            state.last = Some(gap.clone());
            return Ok(gap);
        }
        if state.active.as_ref().is_some_and(|active| active.offset == active.encoded.text.len()) {
            state.active = None;
        }
        if state.active.is_none() {
            match state.receiver.try_recv() {
                Ok(encoded) => state.active = Some(Active { encoded, offset: 0 }),
                Err(watch_queue::TryRecvError::Lagged) => {
                    state.next += 1;
                    let gap = Read::Gap { number: state.next };
                    state.last = Some(gap.clone());
                    return Ok(gap);
                }
                Err(watch_queue::TryRecvError::Empty) => {
                    state.notified = false;
                    return Ok(if state.finished { Read::Closed } else { Read::Idle });
                }
                Err(watch_queue::TryRecvError::Disconnected) => return Ok(Read::Closed),
            }
        }
        state.next += 1;
        let number = state.next;
        let active = state.active.as_mut().expect("packet ready");
        let offset = active.offset;
        let total = active.encoded.text.len();
        let mut end = (offset + CHUNK).min(total);
        while !active.encoded.text.is_char_boundary(end) {
            end -= 1;
        }
        let text = active.encoded.text[offset..end].to_string();
        active.offset = end;
        let part = Read::Chunk { number, offset, total, text };
        state.last = Some(part.clone());
        Ok(part)
    }
}

pub(crate) struct Sender<T> {
    mailbox: Arc<Mailbox>,
    generation: u64,
    sender: watch_queue::Sender<Encoded>,
    key: fn(&T) -> Option<String>,
    pub(super) failed: Arc<AtomicBool>,
}
impl<T: Serialize> Sender<T> {
    pub fn send(&self, piece: T) -> Result<(), ()> {
        let result = (|| {
            let size = watch_queue::size(&piece);
            if size > MAX_PACKET {
                return Err(());
            }
            // A small event does not reserve a full chunk; only one reply is
            // retained beside the packet, with another copy returned to IPC.
            let memory =
                Reservation::new(size.saturating_mul(2).saturating_add(size.min(CHUNK) * 2)).map_err(|_| ())?;
            let text = serde_json::to_string(&piece).map_err(|_| ())?;
            self.sender.send(Encoded { text, key: (self.key)(&piece), _memory: memory }).map_err(|_| ())
        })();
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
            self.sender.lag();
        }
        let mut state = self.mailbox.state.lock().expect("WebView mailbox");
        if state.generation != self.generation {
            return Err(());
        }
        self.mailbox.notify(&mut state)?;
        result
    }
}
