use super::Push;
use fastctx_net::{reassembly::Reservation, watch_queue};
use serde::Serialize;
use serde_json::value::RawValue;
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;

/// The largest piece the page is handed.
const MAX_PACKET: usize = 64 * 1024 * 1024;
/// What the page may hold that it has not acknowledged; past it, the shell
/// waits for an acknowledgement. One piece larger than this still goes, alone.
const WINDOW: usize = 4 * 1024 * 1024;
/// The most one batch gathers of what waits.
const BATCH: usize = 512 * 1024;
/// How soon after a batch the next goes: a burst of pieces goes as a few
/// batches a frame apart, the first piece after a quiet spell at once.
const SPACING: Duration = Duration::from_millis(8);

struct Encoded {
    text: String,
    key: Option<String>,
    _memory: Reservation,
}

/// A batch the page has not acknowledged yet, kept to be pushed again should
/// the page ask (`resend`).
struct Unacknowledged {
    number: u64,
    bytes: usize,
    pieces: Vec<Encoded>,
}

struct State {
    sender: Option<watch_queue::Sender<Encoded>>,
    receiver: watch_queue::Receiver<Encoded>,
    generation: u64,
    /// The number of the last batch, gap or close pushed.
    next: u64,
    unacknowledged: VecDeque<Unacknowledged>,
    held: usize,
    /// A gap pushed and not yet acknowledged: nothing more goes until it is.
    gap: Option<u64>,
    last_push: Option<Instant>,
    scheduled: bool,
    closed: bool,
    finished: bool,
    told_closed: bool,
}

pub(crate) struct Mailbox {
    id: u64,
    channel: Channel<Push>,
    state: Mutex<State>,
    restart: tokio::sync::watch::Sender<u64>,
}

fn queue() -> (watch_queue::Sender<Encoded>, watch_queue::Receiver<Encoded>) {
    watch_queue::channel(|value: &Encoded| (value.text.len(), value.key.clone()))
}

impl Mailbox {
    pub fn new(id: u64, channel: Channel<Push>) -> Arc<Self> {
        let (sender, receiver) = queue();
        Arc::new(Self {
            id,
            channel,
            state: Mutex::new(State {
                sender: Some(sender),
                receiver,
                generation: 0,
                next: 0,
                unacknowledged: VecDeque::new(),
                held: 0,
                gap: None,
                last_push: None,
                scheduled: false,
                closed: false,
                finished: false,
                told_closed: false,
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
        state.unacknowledged.clear();
        state.held = 0;
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
    pub fn finish(self: &Arc<Self>, generation: u64) {
        let mut state = self.state.lock().expect("WebView mailbox");
        if state.generation == generation {
            state.finished = true;
            state.sender.take();
            self.push(&mut state);
        }
    }

    /// The page ran every batch up to `through`: what it holds is let go of,
    /// and more goes. A gap acknowledged starts the source again from what is
    /// now. Whether the stream is over, its close told and taken.
    pub fn acknowledge(self: &Arc<Self>, through: u64) -> Result<bool, String> {
        let mut state = self.state.lock().expect("WebView mailbox");
        if state.closed {
            return Ok(true);
        }
        if through > state.next {
            return Err("the page acknowledged an observation batch not yet sent".into());
        }
        while state.unacknowledged.front().is_some_and(|batch| batch.number <= through) {
            let batch = state.unacknowledged.pop_front().expect("a batch");
            state.held -= batch.bytes;
        }
        if state.gap.is_some_and(|gap| gap <= through) {
            state.gap = None;
            let (sender, receiver) = queue();
            state.sender = Some(sender);
            state.receiver = receiver;
            state.finished = false;
            state.generation += 1;
            self.restart.send_replace(state.generation);
        }
        self.push(&mut state);
        Ok(state.told_closed && state.unacknowledged.is_empty())
    }

    /// The page holds batches up to `through` and missed what came after:
    /// those it has not acknowledged go again, and the next after them.
    pub fn resend(self: &Arc<Self>, through: u64) -> Result<(), String> {
        let mut state = self.state.lock().expect("WebView mailbox");
        if state.closed {
            return self.channel.send(Push::Closed { id: self.id }).map_err(|error| error.to_string());
        }
        // The page holds the gap it asks past: its acknowledgement was lost
        // on the way, and this is it.
        if state.gap.is_some_and(|gap| gap <= through) {
            drop(state);
            return self.acknowledge(through).map(|_| ());
        }
        for batch in state.unacknowledged.iter().filter(|batch| batch.number > through) {
            if self.channel.send(Push::pieces(self.id, batch.number, &batch.pieces)).is_err() {
                break;
            }
        }
        if let Some(gap) = state.gap.filter(|gap| *gap > through) {
            let _ = self.channel.send(Push::Gap { id: self.id, number: gap });
        }
        self.push(&mut state);
        Ok(())
    }

    /// Pushes what waits, as far as the window allows, a batch at a time; a
    /// batch soon after the last waits for `SPACING`, gathering what comes.
    fn push(self: &Arc<Self>, state: &mut State) {
        if state.closed || state.gap.is_some() || state.scheduled {
            return;
        }
        if let Some(last) = state.last_push
            && last.elapsed() < SPACING
        {
            state.scheduled = true;
            let (mailbox, wait) = (self.clone(), SPACING - last.elapsed());
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(wait).await;
                let mut state = mailbox.state.lock().expect("WebView mailbox");
                state.scheduled = false;
                mailbox.push(&mut state);
            });
            return;
        }
        let mut pieces = Vec::new();
        let mut bytes = 0;
        loop {
            if !pieces.is_empty() && (bytes >= BATCH || state.held + bytes >= WINDOW) {
                break;
            }
            if pieces.is_empty() && state.held >= WINDOW {
                return;
            }
            match state.receiver.try_recv() {
                Ok(encoded) => {
                    bytes += encoded.text.len();
                    pieces.push(encoded);
                }
                Err(watch_queue::TryRecvError::Lagged) => {
                    // What waited is gone: the page shows it fell behind, and
                    // the source starts again once it says so.
                    state.next += 1;
                    state.gap = Some(state.next);
                    state.last_push = Some(Instant::now());
                    if self.channel.send(Push::Gap { id: self.id, number: state.next }).is_err() {
                        self.close_state(state);
                    }
                    return;
                }
                Err(watch_queue::TryRecvError::Empty) => break,
                Err(watch_queue::TryRecvError::Disconnected) => {
                    state.finished = true;
                    break;
                }
            }
        }
        if !pieces.is_empty() {
            state.next += 1;
            let number = state.next;
            state.last_push = Some(Instant::now());
            if self.channel.send(Push::pieces(self.id, number, &pieces)).is_err() {
                self.close_state(state);
                return;
            }
            state.held += bytes;
            state.unacknowledged.push_back(Unacknowledged { number, bytes, pieces });
            // More may wait than one batch took.
            if state.held < WINDOW {
                state.scheduled = true;
                let mailbox = self.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(SPACING).await;
                    let mut state = mailbox.state.lock().expect("WebView mailbox");
                    state.scheduled = false;
                    mailbox.push(&mut state);
                });
            }
            return;
        }
        if state.finished && !state.told_closed && state.unacknowledged.is_empty() {
            state.told_closed = true;
            let _ = self.channel.send(Push::Closed { id: self.id });
        }
    }
}

impl Push {
    fn pieces(id: u64, number: u64, pieces: &[Encoded]) -> Self {
        let pieces = pieces
            .iter()
            .filter_map(|piece| RawValue::from_string(piece.text.clone()).ok())
            .collect();
        Push::Pieces { id, number, pieces }
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
    /// Queues a piece for the page, serialized once: what goes to the page is
    /// these same bytes.
    pub fn send(&self, piece: T) -> Result<(), ()> {
        let result = (|| {
            let text = serde_json::to_string(&piece).map_err(|_| ())?;
            if text.len() > MAX_PACKET {
                return Err(());
            }
            // The queued text, and the copy a batch hands the page.
            let memory = Reservation::new(text.len().saturating_mul(2)).map_err(|_| ())?;
            self.sender.send(Encoded { text, key: (self.key)(&piece), _memory: memory }).map_err(|_| ())
        })();
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
            self.sender.lag();
        }
        let mut state = self.mailbox.state.lock().expect("WebView mailbox");
        if state.generation != self.generation || state.closed {
            return Err(());
        }
        self.mailbox.push(&mut state);
        result
    }
}
