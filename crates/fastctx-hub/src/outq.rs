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
//! Within the first lane, what fits one frame (a pong, a call to take, a
//! status, a short answer) goes before the parts of an answer too large for
//! one: a stop or a word for a conversation never waits behind megabytes of
//! someone's history. The parts of one answer keep their order.
//!
//! Each lane is bounded where things are queued. The first by count and
//! bytes, and notices by count. Status notices own bounded cursors into
//! shared immutable snapshots, plus at most one frame and one pending item
//! while draining; they never encode the whole snapshot into this queue.
//! Snapshot bodies belong to the current status and reader baselines, not
//! the cursor budget. Letters are bounded by the mailbox's window.

mod afoot;
mod versions;
pub(crate) use versions::Snapshot as VersionSnapshot;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use fastctx_net::wire::{Down, Refusal};
use fastctx_types::MachineName;
use fastctx_types::access::TownId;
use tokio::sync::{Notify, watch};

use crate::Out;

mod answers;

/// The most frames of the first lane that may wait for a member.
const URGENT: usize = 4096;
/// The most bytes of the first lane that may wait for a member. A full-size
/// answer's JSON text can double when escaped; leave room for its envelopes.
const URGENT_BYTES: usize = 2 * fastctx_net::wire::PARTS_AT_MOST + fastctx_net::wire::MAX_CONTROL_FRAME;
/// The most notices that may wait for a member: one of a kind and subject
/// merges with the next, so only a member that reads nothing piles them up.
const NOTICES: usize = 16384;
/// How many frames of the first lane go in a row while letters or notices
/// wait.
const BURST: usize = 32;

/// What the first lanes of every member together may hold: an eighth of the
/// machine's memory, from 128 MiB to 2 GiB. Each member's lane is bound on
/// its own (`URGENT_BYTES`); ten thousand members each near that bound would
/// hold hundreds of gigabytes (`specs/world-foundation.md`, 宪法第三条). A
/// member whose answer finds no room is let go, as one that took too little
/// of its own lane is, and connects again.
struct Pool {
    used: std::sync::atomic::AtomicUsize,
    bytes: usize,
    /// How much of it was used, told in the hub's health.
    noted: &'static fastctx_platform::health::Bound,
}

fn pool() -> &'static Pool {
    static POOL: std::sync::OnceLock<Pool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let physical = fastctx_platform::memory::physical().unwrap_or(1 << 30);
        Pool {
            used: std::sync::atomic::AtomicUsize::new(0),
            bytes: usize::try_from((physical / 8).clamp(128 << 20, 2 << 30)).unwrap_or(usize::MAX),
            noted: fastctx_platform::health::bound("hub.output"),
        }
    })
}

impl Pool {
    /// Takes `bytes` of it; whether there was room.
    fn take(&self, bytes: usize) -> bool {
        use std::sync::atomic::Ordering;
        let taken = self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|after| *after <= self.bytes)
            })
            .is_ok();
        self.noted.note(self.used.load(Ordering::Relaxed), self.bytes);
        taken
    }

    fn give(&self, bytes: usize) {
        self.used.fetch_sub(bytes, std::sync::atomic::Ordering::AcqRel);
    }
}

/// The part of the pool a member's first lane holds, given back when the
/// lane lets its frames go, however it does.
#[derive(Default)]
struct Charge(usize);

impl Drop for Charge {
    fn drop(&mut self) {
        pool().give(self.0);
    }
}

/// What a member's writer is to do next.
pub(crate) enum Next {
    /// Write these, in order.
    Frames(Vec<Down>),
    /// Say why, then close.
    End(Refusal),
    /// The member left too much untaken: let it go, saying this.
    Overflowed(String),
}

/// How much waits in a queue, lane by lane.
#[derive(Clone, Copy)]
pub(crate) struct Sizes {
    pub urgent: usize,
    pub urgent_bytes: usize,
    pub letters: usize,
    pub notices: usize,
}

/// One member session's queue; clones share it.
#[derive(Clone)]
pub(crate) struct Outq(Arc<Shared>);

struct Shared {
    state: Mutex<State>,
    ready: Notify,
    /// Set once the session's writer is done.
    shut: watch::Sender<bool>,
    /// Why the member is let go, once so much waits for it untaken.
    over: watch::Sender<Option<String>>,
    /// Whether the member reads an answer too large for one frame in parts
    /// (`fastctx_net::wire::Down::Part`).
    parts: bool,
}

#[derive(Default)]
struct State {
    /// Nothing more is taken: the session's writer is done.
    shut: bool,
    welcome: Option<Down>,
    /// The first lane's frames that are whole on their own.
    prompt: VecDeque<(Out, usize)>,
    /// The first lane's parts of large answers, and its end.
    urgent: VecDeque<(Out, usize)>,
    urgent_bytes: usize,
    /// `urgent_bytes`, as taken of every member's pool.
    charged: Charge,
    letters: VecDeque<Down>,
    notices: VecDeque<(Notice, Pending)>,
    /// The afoot notice taken last, drained a frame at a time before the next.
    spilled: Option<afoot::Pending>,
    afoot_bytes: usize,
    afoot_started: bool,
    initial_afoots: usize,
    afoot_ready: bool,
    overflowed: Option<String>,
    /// How many frames of the first lane went in a row.
    in_a_row: usize,
    /// How many whole frames went in a row while parts of a large answer
    /// waited: past `PROMPT_RUN`, a part goes, so it is never starved.
    prompt_run: usize,
    /// Whether letters go before notices next.
    letters_next: bool,
}

/// What a notice is of: a newer one of the same replaces it, or merges with
/// it.
#[derive(PartialEq, Eq)]
enum Notice {
    Replicas,
    History(TownId, String),
    Directory,
    Ledger(TownId),
    AccountRecord(fastctx_types::AccountName, String, String),
    Versions,
    Afoot(MachineName),
    Presence(MachineName),
    Forget(u64),
    Fetch(u64),
    PackageProgress(u64),
    /// A stream moved, or a checkpoint covers a scope: the latest stands.
    Assets(String),
}

enum Pending {
    Frame(Down),
    Afoot(afoot::Pending),
    Versions(versions::Pending),
}

impl Outq {
    /// A queue for a member that reads an answer too large for one frame in
    /// parts, or not.
    pub(crate) fn new(parts: bool) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::default(),
            ready: Notify::new(),
            shut: watch::channel(false).0,
            over: watch::channel(None).0,
            parts,
        }))
    }

    /// Queues the welcome, which goes before anything else.
    pub(crate) fn welcome(&self, welcome: Down) {
        self.0.state.lock().expect("never poisoned").welcome = Some(welcome);
        self.0.ready.notify_one();
    }

    /// Queues `out`; an error once the session's writer is done, or when
    /// the member left so much untaken that it is let go.
    pub(crate) fn send(&self, out: Out) -> Result<(), ()> {
        let frames = match out {
            Out::Frame(Down::Told { id, answer }) => {
                answers::frames(id, answer, self.0.parts).into_iter().map(Out::Frame).collect()
            }
            out => vec![out],
        };
        let whole = frames.len() == 1;
        let frames: Vec<_> = frames
            .into_iter()
            .map(|out| {
                let bytes = match &out {
                    Out::Frame(frame) => fastctx_net::wire::encode(frame).len(),
                    Out::End(_) => 0,
                };
                (out, bytes)
            })
            .collect();
        let (queued, overflowed) = {
            let mut state = self.0.state.lock().expect("never poisoned");
            if state.shut || state.overflowed.is_some() {
                return Err(());
            }
            let queued = frames.into_iter().try_for_each(|(out, bytes)| match out {
                Out::Frame(Down::Deliver { seq, message }) => {
                    state.letters.push_back(Down::Deliver { seq, message });
                    Ok(())
                }
                Out::Frame(frame) => match notice(&frame) {
                    Some(notice) => state.notify(notice, frame),
                    None => state.urge(Out::Frame(frame), bytes, whole),
                },
                end @ Out::End(_) => state.urge(end, bytes, false),
            });
            (queued, state.overflowed.clone())
        };
        // Let go: a write under way for it stops too (`overflowed`).
        if let Some(why) = overflowed {
            self.0.over.send_replace(Some(why));
        }
        self.0.ready.notify_one();
        queued
    }

    /// Publishes the latest version snapshot, one frame per turn in the notice lane.
    pub(crate) fn versions(&self, snapshot: VersionSnapshot, parts: bool) -> Result<(), ()> {
        let mut state = self.0.state.lock().expect("never poisoned");
        if state.shut || state.overflowed.is_some() {
            return Err(());
        }
        let pending = Pending::Versions(versions::Pending::new(snapshot, parts));
        if let Some(at) = state.notices.iter().position(|(kind, _)| *kind == Notice::Versions) {
            state.notices[at].1 = pending;
        } else if state.notices.len() < NOTICES {
            state.notices.push_back((Notice::Versions, pending));
        } else {
            let _ = state.overflow("too many pending notices".into());
            self.0.over.send_replace(state.overflowed.clone());
            self.0.ready.notify_one();
            return Err(());
        }
        self.0.ready.notify_one();
        Ok(())
    }

    /// Shares a status snapshot and only materializes a frame when its writer asks.
    pub(crate) fn afoot(
        &self,
        machine: MachineName,
        before: Option<Arc<fastctx_types::town::AfootSeen>>,
        after: Arc<fastctx_types::town::AfootSeen>,
        busy: Option<u32>,
    ) -> Result<(), ()> {
        let (result, overflowed) = {
            let mut state = self.0.state.lock().expect("never poisoned");
            if state.shut || state.overflowed.is_some() {
                return Err(());
            }
            let notice = Notice::Afoot(machine.clone());
            let at = state.notices.iter().position(|(key, _)| *key == notice);
            let result = if let Some(at) = at {
                let Pending::Afoot(pending) = &mut state.notices[at].1 else { unreachable!("status snapshot") };
                pending.replace(after, busy);
                Ok(())
            } else if state.notices.len() >= NOTICES {
                state.overflow("too many pending notices".into())
            } else {
                match afoot::Pending::new(machine, before, after, busy, &mut state.afoot_bytes) {
                    Ok(mut pending) => {
                        pending.initial = !state.afoot_started;
                        state.initial_afoots += usize::from(pending.initial);
                        state.notices.push_back((notice, Pending::Afoot(pending)));
                        Ok(())
                    }
                    Err(why) => state.afoot_overflow(why),
                }
            };
            (result, state.overflowed.clone())
        };
        if let Some(why) = overflowed {
            self.0.over.send_replace(Some(why));
        }
        self.0.ready.notify_one();
        result
    }

    /// Initial producers are finished; the writer acknowledges after draining their frames.
    pub(crate) fn afoot_started(&self) {
        self.0.state.lock().expect("never poisoned").afoot_started = true;
        self.0.ready.notify_one();
    }

    /// What to write next; none once the queue is shut.
    pub(crate) async fn next(&self) -> Option<Next> {
        loop {
            // Asked for before looking, so a frame queued in between wakes it.
            let ready = self.0.ready.notified();
            let queue = self.clone();
            let next = fastctx_platform::task::spawn_blocking(move || {
                let mut state = queue.0.state.lock().expect("never poisoned");
                if state.shut { Some(None) } else { state.take().map(Some) }
            })
            .await;
            match next {
                Ok(Some(next)) => return next,
                Ok(None) => {}
                Err(error) => return Some(Next::Overflowed(format!("preparing a queued frame: {error}"))),
            }
            ready.await;
        }
    }

    /// The session's writer is done: nothing more is taken, and whoever
    /// waits for the session to end hears it.
    pub(crate) fn shut(&self) {
        let old = std::mem::replace(
            &mut *self.0.state.lock().expect("never poisoned"),
            State { shut: true, ..State::default() },
        );
        drop(old);
        self.0.shut.send_replace(true);
        self.0.ready.notify_waiters();
    }

    /// How much waits in it now.
    pub(crate) fn sizes(&self) -> Sizes {
        let state = self.0.state.lock().expect("never poisoned");
        Sizes {
            urgent: state.prompt.len() + state.urgent.len(),
            urgent_bytes: state.urgent_bytes,
            letters: state.letters.len(),
            notices: state.notices.len() + usize::from(state.spilled.is_some()),
        }
    }

    /// Resolves, with why, once so much waits for the member untaken that it
    /// is let go: the writer stops a write under way then, however long that
    /// would still take.
    pub(crate) async fn overflowed(&self) -> String {
        let mut over = self.0.over.subscribe();
        let why = over.wait_for(Option::is_some).await.ok().map(|why| why.clone().unwrap_or_default());
        match why {
            Some(why) => why,
            // The queue outlives its writer, which is all that waits on this.
            None => std::future::pending().await,
        }
    }

    /// Identifies this connection while a clone keeps its queue alive.
    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    /// Resolves once the queue is shut.
    pub(crate) async fn closed(&self) {
        let mut shut = self.0.shut.subscribe();
        let _ = shut.wait_for(|shut| *shut).await;
    }
}

/// How many whole frames of the first lane go in a row before a waiting part
/// of a large answer.
const PROMPT_RUN: usize = 16;

impl State {
    /// The first lane's next frame: whole ones first, a part after every
    /// `PROMPT_RUN` of them.
    fn first_lane(&mut self) -> Option<(Out, usize)> {
        if self.prompt_run >= PROMPT_RUN
            && let Some(part) = self.urgent.pop_front()
        {
            self.prompt_run = 0;
            return Some(part);
        }
        if let Some(whole) = self.prompt.pop_front() {
            self.prompt_run += 1;
            return Some(whole);
        }
        self.prompt_run = 0;
        self.urgent.pop_front()
    }

    fn overflow(&mut self, why: String) -> Result<(), ()> {
        self.overflowed = Some(why);
        Err(())
    }

    fn urge(&mut self, out: Out, bytes: usize, whole: bool) -> Result<(), ()> {
        let waiting = self.prompt.len() + self.urgent.len();
        if waiting >= URGENT || self.urgent_bytes + bytes > URGENT_BYTES {
            let why = format!(
                "{waiting} frames of {} bytes wait for it untaken, as many as {URGENT} or {URGENT_BYTES} bytes; letting it go",
                self.urgent_bytes
            );
            return self.overflow(why);
        }
        if !pool().take(bytes) {
            let why = format!(
                "what waits for every member takes as much as the hub keeps for them, {} bytes; letting it go",
                pool().bytes
            );
            return self.overflow(why);
        }
        self.charged.0 += bytes;
        self.urgent_bytes += bytes;
        if whole { self.prompt.push_back((out, bytes)) } else { self.urgent.push_back((out, bytes)) }
        Ok(())
    }

    fn notify(&mut self, notice: Notice, frame: Down) -> Result<(), ()> {
        let at = self.notices.iter().position(|(kept, _)| *kept == notice);
        if at.is_none() && self.notices.len() >= NOTICES {
            return self.overflow(format!("{NOTICES} notices wait for it untaken; letting it go"));
        }
        if matches!(frame, Down::Afoot { .. }) {
            return self.overflow("status changes must retain their owning snapshots".into());
        }
        match at {
            Some(at) if older_presence(&self.notices[at].1, &frame) => {}
            Some(at) => self.notices[at].1 = Pending::Frame(frame),
            None => self.notices.push_back((notice, Pending::Frame(frame))),
        }
        Ok(())
    }

    fn afoot_overflow(&mut self, why: &str) -> Result<(), ()> {
        self.overflow(format!("{why}: {} bytes charged, limit {}; letting it go", self.afoot_bytes, afoot::LIMIT))
    }

    fn take(&mut self) -> Option<Next> {
        if let Some(welcome) = self.welcome.take() {
            return Some(Next::Frames(vec![welcome]));
        }
        if let Some(why) = &self.overflowed {
            return Some(Next::Overflowed(why.clone()));
        }
        if self.afoot_started && self.initial_afoots == 0 && !self.afoot_ready {
            self.afoot_ready = true;
            return Some(Next::Frames(vec![Down::Pong { n: 0, at: None, took: None, afoot_ready: true }]));
        }
        let others = !self.letters.is_empty() || !self.notices.is_empty() || self.spilled.is_some();
        if (self.in_a_row < BURST || !others)
            && let Some((out, bytes)) = self.first_lane()
        {
            self.urgent_bytes -= bytes;
            self.charged.0 -= bytes;
            pool().give(bytes);
            self.in_a_row += 1;
            return Some(match out {
                Out::Frame(frame) => Next::Frames(vec![frame]),
                Out::End(reason) => Next::End(reason),
            });
        }
        self.in_a_row = 0;
        let letter = |state: &mut Self| state.letters.pop_front();
        let notice = |state: &mut Self| {
            if state.spilled.is_none() {
                match state.notices.pop_front()?.1 {
                    Pending::Frame(frame) => return Some(frame),
                    Pending::Versions(mut pending) => match pending.take() {
                        Ok((frame, done)) => {
                            if !done {
                                state.notices.push_back((Notice::Versions, Pending::Versions(pending)));
                            }
                            return Some(frame);
                        }
                        Err(reason) => return Some(Down::Refused { reason: Refusal::Broken { reason } }),
                    },
                    Pending::Afoot(pending) => state.spilled = Some(pending),
                }
            }
            let pending = state.spilled.as_mut().expect("taken above");
            let (frame, done) = match pending.take(&mut state.afoot_bytes) {
                Ok(taken) => taken,
                Err(why) => {
                    state.overflowed = Some(why);
                    return None;
                }
            };
            if done {
                state.initial_afoots -= usize::from(pending.initial);
                state.spilled = None;
            }
            Some(frame)
        };
        self.letters_next = !self.letters_next;
        let frame = if self.letters_next {
            letter(self).or_else(|| notice(self))
        } else {
            notice(self).or_else(|| letter(self))
        };
        if let Some(why) = &self.overflowed {
            return Some(Next::Overflowed(why.clone()));
        }
        frame.map(|frame| Next::Frames(vec![frame]))
    }
}

/// Whether `frame` tells of a presence counted before the one `kept` tells of.
fn older_presence(kept: &Pending, frame: &Down) -> bool {
    matches!(
        (kept, frame),
        (Pending::Frame(Down::Presence { rev: kept, .. }), Down::Presence { rev, .. }) if rev < kept
    )
}

/// The notice `frame` is, when it is one.
fn notice(frame: &Down) -> Option<Notice> {
    Some(match frame {
        Down::DirectoryChanged => Notice::Directory,
        Down::ReplicasChanged => Notice::Replicas,
        Down::HistoryChanged { town, name, .. } => Notice::History(town.clone(), name.clone()),
        Down::LedgerChanged { town } => Notice::Ledger(town.clone()),
        Down::AccountRecordChanged { changed } => {
            Notice::AccountRecord(changed.account.clone(), changed.metadata.kind.clone(), changed.metadata.key.clone())
        }
        Down::Versions { .. } => Notice::Versions,
        Down::Afoot { machine, .. } => Notice::Afoot(machine.clone()),
        Down::Presence { machine, .. } => Notice::Presence(machine.clone()),
        Down::Forget { forgetting } => Notice::Forget(forgetting.id),
        Down::Fetch { fetch } => Notice::Fetch(fetch.id),
        Down::FetchCancelled { id } => Notice::Fetch(*id),
        Down::PackageProgress { id, .. } => Notice::PackageProgress(*id),
        Down::Assets { push } => match push {
            fastctx_types::assets::Push::Moved { scope, stream, .. } => Notice::Assets(format!("moved\0{scope}\0{stream}")),
            fastctx_types::assets::Push::Checkpointed { checkpoint } => {
                Notice::Assets(format!("checkpointed\0{}", checkpoint.scope))
            }
            fastctx_types::assets::Push::Revoked { scope } => Notice::Assets(format!("revoked\0{scope}")),
            fastctx_types::assets::Push::Fact { .. } => return None,
        },
        _ => return None,
    })
}
