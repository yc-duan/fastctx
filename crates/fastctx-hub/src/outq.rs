//! What waits to go to one member on its control connection, and in what
//! order (`world-concurrency.md`, 机器之间).
//!
//! Three lanes. What answers or ends something goes first: pongs, answers,
//! the calls the member is to take, statuses, receipts, refusals. Then, in
//! turn, letters, which the member's mailbox puts here a window at a time
//! (`crate::store`, `Mailbox`), and notices that a newer one replaces or
//! merges with: the directory changed, a project's ledger changed, the
//! network's versions, what is afoot on a machine, a conversation to
//! delete, a package to fetch.
//!
//! Each lane is bounded. The first by count and bytes: a member that
//! leaves more than `URGENT` frames or `URGENT_BYTES` bytes of it untaken
//! does not read what it is sent, and is let go; it comes back and reads on
//! from where it was. Letters are bounded by the mailbox's window, notices
//! by having one of each kind and subject. The welcome is the first frame a
//! session writes, so no backlog keeps a member from coming in, and a pong
//! never waits behind letters.

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
    /// Set once the session's writer is done: nothing more is taken.
    shut: watch::Sender<bool>,
}

#[derive(Default)]
struct State {
    urgent: VecDeque<(Out, usize)>,
    urgent_bytes: usize,
    letters: VecDeque<Down>,
    notices: Vec<(Notice, Down)>,
    overflowed: Option<String>,
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
    Forget(u64),
    Fetch(u64),
}

impl Outq {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Shared { state: Mutex::default(), ready: Notify::new(), shut: watch::channel(false).0 }))
    }

    /// Queues `out`; an error once the session's writer is done.
    pub(crate) fn send(&self, out: Out) -> Result<(), ()> {
        if *self.0.shut.borrow() {
            return Err(());
        }
        {
            let mut state = self.0.state.lock().expect("never poisoned");
            match out {
                Out::Frame(Down::Deliver { seq, message }) => state.letters.push_back(Down::Deliver { seq, message }),
                Out::Frame(frame) => match notice(&frame) {
                    Some(notice) => state.notify(notice, frame),
                    None => state.urge(Out::Frame(frame)),
                },
                end @ Out::End(_) => state.urge(end),
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
            if *self.0.shut.borrow() {
                return None;
            }
            if let Some(next) = self.0.state.lock().expect("never poisoned").take() {
                return Some(next);
            }
            ready.await;
        }
    }

    /// The session's writer is done: nothing more is taken, and whoever
    /// waits for the session to end hears it.
    pub(crate) fn shut(&self) {
        self.0.shut.send_replace(true);
        self.0.ready.notify_waiters();
        *self.0.state.lock().expect("never poisoned") = State::default();
    }

    /// Resolves once the queue is shut.
    pub(crate) async fn closed(&self) {
        let mut shut = self.0.shut.subscribe();
        let _ = shut.wait_for(|shut| *shut).await;
    }
}

impl State {
    fn urge(&mut self, out: Out) {
        let bytes = match &out {
            Out::Frame(frame) => fastctx_net::wire::encode(frame).len(),
            Out::End(_) => 0,
        };
        self.urgent_bytes += bytes;
        self.urgent.push_back((out, bytes));
        if self.overflowed.is_none() && (self.urgent.len() > URGENT || self.urgent_bytes > URGENT_BYTES) {
            self.overflowed = Some(format!(
                "{} frames of {} bytes wait for it untaken, more than {URGENT} or {URGENT_BYTES} bytes; letting it go",
                self.urgent.len(),
                self.urgent_bytes
            ));
        }
    }

    fn notify(&mut self, notice: Notice, frame: Down) {
        let Some(at) = self.notices.iter().position(|(kept, _)| *kept == notice) else {
            self.notices.push((notice, frame));
            return;
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
    }

    fn take(&mut self) -> Option<Next> {
        if let Some(why) = self.overflowed.take() {
            return Some(Next::Overflowed(why));
        }
        if let Some((out, bytes)) = self.urgent.pop_front() {
            self.urgent_bytes -= bytes;
            return Some(match out {
                Out::Frame(frame) => Next::Frames(vec![frame]),
                Out::End(reason) => Next::End(reason),
            });
        }
        let letter = |state: &mut Self| state.letters.pop_front().map(|frame| Next::Frames(vec![frame]));
        let notice = |state: &mut Self| {
            if state.notices.is_empty() {
                return None;
            }
            let (_, frame) = state.notices.remove(0);
            Some(Next::Frames(split(frame)))
        };
        self.letters_next = !self.letters_next;
        if self.letters_next { letter(self).or_else(|| notice(self)) } else { notice(self).or_else(|| letter(self)) }
    }
}

/// The notice `frame` is, when it is one.
fn notice(frame: &Down) -> Option<Notice> {
    Some(match frame {
        Down::DirectoryChanged => Notice::Directory,
        Down::LedgerChanged { town } => Notice::Ledger(town.clone()),
        Down::Versions { .. } => Notice::Versions,
        Down::Afoot { machine, .. } => Notice::Afoot(machine.clone()),
        Down::Forget { forgetting } => Notice::Forget(forgetting.id),
        Down::Fetch { fetch } => Notice::Fetch(fetch.id),
        _ => return None,
    })
}

/// A notice as the frames it goes in: what is afoot on a machine, merged
/// from several, may need more than one.
fn split(frame: Down) -> Vec<Down> {
    let Down::Afoot { machine, busy, change } = frame else { return vec![frame] };
    let (changes, _) = fastctx_net::afoot::frames(*change, fastctx_net::afoot::FRAME_ROOM);
    let changes = if changes.is_empty() { vec![Default::default()] } else { changes };
    changes.into_iter().map(|change| Down::Afoot { machine: machine.clone(), busy, change: Box::new(change) }).collect()
}
