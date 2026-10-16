//! What waits to go to one member on its control connection, and in what
//! order (`world-concurrency.md`, 机器之间).
//!
//! The welcome first, always: no backlog keeps a member from coming in.
//! Then three lanes. What answers or ends something goes first: pongs,
//! answers, the calls the member is to take, statuses, receipts, refusals.
//! Then, in turn, letters, which the member's mailbox puts here a window at
//! a time (`crate::mailbox`), and notices that a newer one replaces or merges
//! with: the directory changed, a project's ledger changed, the network's
//! versions, what is afoot on a machine, a conversation to delete, a package
//! to fetch. The first lane has `BURST` frames in a row at most before a
//! letter or a notice has its turn, so neither waits for long behind it; a
//! notice that takes several frames goes a frame at a turn, so a pong never
//! waits behind all of it.
//!
//! Each lane is bounded where things are queued. The first by count and
//! bytes, and notices by count: a member that leaves more than that
//! untaken does not read what it is sent, and is let go; it comes back and
//! reads on from where it was. Letters are bounded by the mailbox's window.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use fastctx_net::wire::{Down, Refusal};
use fastctx_types::MachineName;
use fastctx_types::access::TownId;
use tokio::sync::{Notify, watch};

use crate::Out;

/// The most frames of the first lane that may wait for a member.
const URGENT: usize = 4096;
/// The most bytes of the first lane that may wait for a member.
const URGENT_BYTES: usize = 32 * 1024 * 1024;
/// The most notices that may wait for a member: one of a kind and subject
/// merges with the next, so only a member that reads nothing piles them up.
const NOTICES: usize = 16384;
/// How many frames of the first lane go in a row while letters or notices
/// wait.
const BURST: usize = 32;

/// What a member's writer is to do next.
pub(crate) enum Next {
    /// Write these, in order.
    Frames(Vec<Down>),
    /// Say why, then close.
    End(Refusal),
    /// The member left too much untaken: let it go, saying this.
    Overflowed(String),
}

/// One member session's queue; clones share it.
#[derive(Clone)]
pub(crate) struct Outq(Arc<Shared>);

struct Shared {
    state: Mutex<State>,
    ready: Notify,
    /// Set once the session's writer is done.
    shut: watch::Sender<bool>,
    /// Whether the member reads an answer too large for one frame in parts
    /// (`fastctx_net::wire::Down::Part`).
    parts: bool,
}

#[derive(Default)]
struct State {
    /// Nothing more is taken: the session's writer is done.
    shut: bool,
    welcome: Option<Down>,
    urgent: VecDeque<(Out, usize)>,
    urgent_bytes: usize,
    letters: VecDeque<Down>,
    notices: VecDeque<(Notice, Down)>,
    /// The frames of the notice taken last that are still to go.
    spilled: VecDeque<Down>,
    overflowed: Option<String>,
    /// How many frames of the first lane went in a row.
    in_a_row: usize,
    /// Whether letters go before notices next.
    letters_next: bool,
}

/// What a notice is of: a newer one of the same replaces it, or merges with
/// it.
#[derive(PartialEq, Eq)]
enum Notice {
    Directory,
    Ledger(TownId),
    Versions,
    Afoot(MachineName),
    Presence(MachineName),
    Forget(u64),
    Fetch(u64),
}

impl Outq {
    /// A queue for a member that reads an answer too large for one frame in
    /// parts, or not.
    pub(crate) fn new(parts: bool) -> Self {
        Self(Arc::new(Shared { state: Mutex::default(), ready: Notify::new(), shut: watch::channel(false).0, parts }))
    }

    /// Queues the welcome, which goes before anything else.
    pub(crate) fn welcome(&self, welcome: Down) {
        self.0.state.lock().expect("never poisoned").welcome = Some(welcome);
        self.0.ready.notify_one();
    }

    /// Queues `out`; an error once the session's writer is done, or when
    /// the member left so much untaken that it is let go.
    pub(crate) fn send(&self, out: Out) -> Result<(), ()> {
        {
            let mut state = self.0.state.lock().expect("never poisoned");
            if state.shut || state.overflowed.is_some() {
                return Err(());
            }
            match out {
                Out::Frame(Down::Deliver { seq, message }) => state.letters.push_back(Down::Deliver { seq, message }),
                Out::Frame(Down::Told { id, answer }) => {
                    for frame in told(id, answer, self.0.parts) {
                        state.urge(Out::Frame(frame))?;
                    }
                }
                Out::Frame(frame) => match notice(&frame) {
                    Some(notice) => state.notify(notice, frame)?,
                    None => state.urge(Out::Frame(frame))?,
                },
                end @ Out::End(_) => state.urge(end)?,
            }
        }
        self.0.ready.notify_one();
        Ok(())
    }

    /// What to write next; none once the queue is shut.
    pub(crate) async fn next(&self) -> Option<Next> {
        loop {
            // Asked for before looking, so a frame queued in between wakes it.
            let ready = self.0.ready.notified();
            {
                let mut state = self.0.state.lock().expect("never poisoned");
                if state.shut {
                    return None;
                }
                if let Some(next) = state.take() {
                    return Some(next);
                }
            }
            ready.await;
        }
    }

    /// The session's writer is done: nothing more is taken, and whoever
    /// waits for the session to end hears it.
    pub(crate) fn shut(&self) {
        *self.0.state.lock().expect("never poisoned") = State { shut: true, ..State::default() };
        self.0.shut.send_replace(true);
        self.0.ready.notify_waiters();
    }

    /// Resolves once the queue is shut.
    pub(crate) async fn closed(&self) {
        let mut shut = self.0.shut.subscribe();
        let _ = shut.wait_for(|shut| *shut).await;
    }
}

impl State {
    fn overflow(&mut self, why: String) -> Result<(), ()> {
        self.overflowed = Some(why);
        Err(())
    }

    fn urge(&mut self, out: Out) -> Result<(), ()> {
        let bytes = match &out {
            Out::Frame(frame) => fastctx_net::wire::encode(frame).len(),
            Out::End(_) => 0,
        };
        if self.urgent.len() >= URGENT || self.urgent_bytes + bytes > URGENT_BYTES {
            let why = format!(
                "{} frames of {} bytes wait for it untaken, as many as {URGENT} or {URGENT_BYTES} bytes; letting it go",
                self.urgent.len(),
                self.urgent_bytes
            );
            return self.overflow(why);
        }
        self.urgent_bytes += bytes;
        self.urgent.push_back((out, bytes));
        Ok(())
    }

    fn notify(&mut self, notice: Notice, frame: Down) -> Result<(), ()> {
        let Some(at) = self.notices.iter().position(|(kept, _)| *kept == notice) else {
            if self.notices.len() >= NOTICES {
                return self.overflow(format!("{NOTICES} notices wait for it untaken; letting it go"));
            }
            self.notices.push_back((notice, frame));
            return Ok(());
        };
        let kept = &mut self.notices[at].1;
        match (kept, frame) {
            (Down::Afoot { change: kept_change, busy: kept_busy, .. }, Down::Afoot { change, busy, .. }) => {
                let earlier = std::mem::take(&mut **kept_change);
                **kept_change = earlier.then(*change);
                *kept_busy = busy;
            }
            (kept, frame) => *kept = frame,
        }
        Ok(())
    }

    fn take(&mut self) -> Option<Next> {
        if let Some(welcome) = self.welcome.take() {
            return Some(Next::Frames(vec![welcome]));
        }
        if let Some(why) = &self.overflowed {
            return Some(Next::Overflowed(why.clone()));
        }
        let others = !self.letters.is_empty() || !self.notices.is_empty() || !self.spilled.is_empty();
        if (self.in_a_row < BURST || !others)
            && let Some((out, bytes)) = self.urgent.pop_front()
        {
            self.urgent_bytes -= bytes;
            self.in_a_row += 1;
            return Some(match out {
                Out::Frame(frame) => Next::Frames(vec![frame]),
                Out::End(reason) => Next::End(reason),
            });
        }
        self.in_a_row = 0;
        let letter = |state: &mut Self| state.letters.pop_front();
        let notice = |state: &mut Self| {
            if state.spilled.is_empty() {
                let (_, frame) = state.notices.pop_front()?;
                state.spilled.extend(split(frame));
            }
            state.spilled.pop_front()
        };
        self.letters_next = !self.letters_next;
        let frame = if self.letters_next {
            letter(self).or_else(|| notice(self))
        } else {
            notice(self).or_else(|| letter(self))
        };
        frame.map(|frame| Next::Frames(vec![frame]))
    }
}

/// The notice `frame` is, when it is one.
fn notice(frame: &Down) -> Option<Notice> {
    Some(match frame {
        Down::DirectoryChanged => Notice::Directory,
        Down::LedgerChanged { town } => Notice::Ledger(town.clone()),
        Down::Versions { .. } => Notice::Versions,
        Down::Afoot { machine, .. } => Notice::Afoot(machine.clone()),
        Down::Presence { machine, .. } => Notice::Presence(machine.clone()),
        Down::Forget { forgetting } => Notice::Forget(forgetting.id),
        Down::Fetch { fetch } => Notice::Fetch(fetch.id),
        _ => return None,
    })
}

/// The answer to the question `id` as the frames it goes in: one when it
/// fits in a frame; else in parts for a member that reads them, and for one
/// that does not, that the answer is too large for its release.
fn told(id: u64, answer: Result<fastctx_net::wire::Told, String>, parts: bool) -> Vec<Down> {
    let whole = Down::Told { id, answer };
    if fastctx_net::wire::encode(&whole).len() <= fastctx_net::wire::MAX_CONTROL_FRAME {
        return vec![whole];
    }
    let Down::Told { answer, .. } = whole else { unreachable!("made above") };
    let text = serde_json::to_string(&answer).expect("answers serialize");
    let too_large = |why: String| vec![Down::Told { id, answer: Err(why) }];
    if !parts {
        return too_large(format!(
            "the hub's answer is {} bytes, more than this release of Fastctx reads in one frame; update it",
            text.len()
        ));
    }
    if text.len() > fastctx_net::wire::PARTS_AT_MOST {
        return too_large(format!(
            "the hub's answer is {} bytes, more than {} it sends in parts",
            text.len(),
            fastctx_net::wire::PARTS_AT_MOST
        ));
    }
    // Cut at character boundaries, each part well within a frame.
    let mut pieces = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let mut end = rest.len().min(fastctx_net::wire::PART);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (piece, after) = rest.split_at(end);
        pieces.push(piece.to_string());
        rest = after;
    }
    let total = u32::try_from(pieces.len()).expect("parts are few");
    pieces.into_iter().enumerate().map(|(index, text)| Down::Part { id, index: index as u32, total, text }).collect()
}

/// A notice as the frames it goes in: what is afoot on a machine, merged
/// from several, may need more than one.
fn split(frame: Down) -> Vec<Down> {
    let Down::Afoot { machine, busy, change } = frame else { return vec![frame] };
    let (changes, _) = fastctx_net::afoot::frames(*change, fastctx_net::afoot::FRAME_ROOM);
    let changes = if changes.is_empty() { vec![Default::default()] } else { changes };
    changes.into_iter().map(|change| Down::Afoot { machine: machine.clone(), busy, change: Box::new(change) }).collect()
}
