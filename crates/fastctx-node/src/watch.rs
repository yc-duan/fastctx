//! Watch publication locks only this conversation's state. Small registry shards
//! locate the conversation and are released before observing or fanning out an
//! event. A slow consumer loses its bounded queue and resumes from history.
//!
//! What is told of a conversation is numbered (`session::Mark`), and its latest
//! events are kept a while after its last viewer left: a viewer that comes
//! back takes only the events after the last it took, while they are kept.

use fastctx_net::watch_queue::{self, Sender};
use fastctx_types::ResidentId;
use fastctx_types::session::{Mark, SessionEvent};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

/// The bytes of the events kept for viewers that come back, all
/// conversations together.
static KEPT: LazyLock<&'static fastctx_platform::readings::Gauge> =
    LazyLock::new(|| fastctx_platform::readings::gauge("watch.kept_bytes"));

const SHARDS: usize = 64;
type Sessions = HashMap<ResidentId, Arc<Mutex<Watching>>>;

/// An event as the watchers tell it, with where it stands in its
/// conversation's telling; a snapshot of what is under way has no place in
/// it.
#[derive(Clone)]
pub(crate) struct Marked {
    pub mark: Option<Mark>,
    pub event: SessionEvent,
}

impl std::ops::Deref for Marked {
    type Target = SessionEvent;

    fn deref(&self) -> &SessionEvent {
        &self.event
    }
}

/// What a watch of a conversation is told, in order.
pub(crate) type Events = watch_queue::Receiver<Arc<Marked>>;

pub(crate) fn events() -> (Sender<Arc<Marked>>, Events) {
    watch_queue::channel(|marked: &Arc<Marked>| watch_queue::event_size(&marked.event))
}

/// How many of a conversation's latest events are kept, and how many bytes
/// of them, for a viewer that comes back to take what it missed.
const KEPT_EVENTS: usize = 4096;
const KEPT_BYTES: usize = 8 << 20;
/// The events every conversation keeps together: past this, the one telling
/// lets its oldest go first.
const KEPT_ALL: usize = 64 << 20;
/// How long after its last viewer left a conversation keeps its latest
/// events for one that comes back.
const KEPT_FOR: Duration = Duration::from_secs(120);

pub struct Watchers {
    watching: Arc<[RwLock<Sessions>; SHARDS]>,
    pub(crate) changed: Arc<Changed>,
    /// The bytes of the events every conversation keeps.
    kept: Arc<AtomicUsize>,
}
impl Default for Watchers {
    fn default() -> Self {
        Self {
            watching: Arc::new(std::array::from_fn(|_| RwLock::default())),
            changed: Arc::default(),
            kept: Arc::default(),
        }
    }
}
/// The conversations whose watching, or turns, changed since the lifecycle
/// last looked (`lifecycle/retention.rs`): between its rounds of everyone it
/// looks at those alone.
#[derive(Default)]
pub(crate) struct Changed {
    ids: Mutex<HashSet<ResidentId>>,
    notify: tokio::sync::Notify,
}

impl Changed {
    pub(crate) fn tell(&self, id: &ResidentId) {
        self.ids.lock().expect("never poisoned").insert(id.clone());
        self.notify.notify_one();
    }

    pub(crate) async fn told(&self) {
        self.notify.notified().await;
    }

    /// Those told of since the last time, let go.
    pub(crate) fn take(&self) -> HashSet<ResidentId> {
        std::mem::take(&mut *self.ids.lock().expect("never poisoned"))
    }
}

#[derive(Default)]
struct Watching {
    senders: Vec<Sender<Arc<Marked>>>,
    /// Those that hear what is told without viewing the conversation, as a
    /// collaboration's keeper does: they wake no host and keep nothing.
    quiet: Vec<Sender<Arc<Marked>>>,
    live: crate::session_live::Live,
    told: Telling,
}

/// What has been told of a conversation since its watching began: how many
/// events, and the latest of them.
struct Telling {
    epoch: u64,
    sequence: u64,
    kept: VecDeque<(u64, Arc<Marked>, usize)>,
    bytes: usize,
    /// Every event from this one on is kept.
    whole_from: u64,
    /// The last event that told the history changed or was cut: a history
    /// read before it is no longer the history.
    reset: u64,
    /// When its last viewer left, while it keeps its events for one that
    /// comes back.
    left: Option<Instant>,
}

impl Default for Telling {
    fn default() -> Self {
        let mut epoch = [0u8; 8];
        getrandom::fill(&mut epoch).expect("the system's random numbers");
        Self {
            epoch: u64::from_le_bytes(epoch),
            sequence: 0,
            kept: VecDeque::new(),
            bytes: 0,
            whole_from: 1,
            reset: 0,
            left: None,
        }
    }
}

impl Telling {
    /// Numbers `event` as the next told and, when `keep`, keeps it, letting
    /// the oldest go past this conversation's bounds or all conversations'
    /// together; one not kept leaves nothing before it to take.
    fn tell(&mut self, event: &SessionEvent, bytes: usize, keep: bool, all: &AtomicUsize) -> Arc<Marked> {
        use fastctx_types::session::{Happened, SessionChange};
        self.sequence += 1;
        if matches!(
            &event.happened,
            Happened::Changed { change: SessionChange::HistoryChanged | SessionChange::HistoryCut { .. }, .. }
        ) {
            self.reset = self.sequence;
        }
        let marked =
            Arc::new(Marked { mark: Some(Mark { epoch: self.epoch, sequence: self.sequence }), event: event.clone() });
        if !keep {
            self.forget(all);
            return marked;
        }
        self.kept.push_back((self.sequence, marked.clone(), bytes));
        self.bytes += bytes;
        all.fetch_add(bytes, Ordering::Relaxed);
        KEPT.add(bytes as i64);
        // Past what all conversations keep together, this one lets go of
        // its own oldest, the event just told too: a viewer coming back
        // then reads the history again, and no bound is passed.
        while self.kept.len() > KEPT_EVENTS || self.bytes > KEPT_BYTES || all.load(Ordering::Relaxed) > KEPT_ALL {
            let Some((sequence, _, gone)) = self.kept.pop_front() else { break };
            self.bytes -= gone;
            all.fetch_sub(gone, Ordering::Relaxed);
            KEPT.add(-(gone as i64));
            self.whole_from = sequence + 1;
        }
        marked
    }

    /// The events after `since`, when every one of them is kept.
    fn after(&self, since: Mark) -> Option<Vec<Arc<Marked>>> {
        (since.epoch == self.epoch && since.sequence + 1 >= self.whole_from && since.sequence <= self.sequence).then(
            || {
                self.kept
                    .iter()
                    .filter(|(sequence, ..)| *sequence > since.sequence)
                    .map(|(_, marked, _)| marked.clone())
                    .collect()
            },
        )
    }

    fn forget(&mut self, all: &AtomicUsize) {
        all.fetch_sub(self.bytes, Ordering::Relaxed);
        KEPT.add(-(self.bytes as i64));
        self.kept.clear();
        self.bytes = 0;
        self.whole_from = self.sequence + 1;
    }
}

/// How a watch is opened: for a viewer, with the snapshots of what is
/// under way first on its events or apart from them, or to hear quietly.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Opened {
    Inline,
    Apart,
    Quiet,
}

/// A watch of a conversation as `Watchers::follow` opens it.
pub(crate) struct Followed {
    /// The events after the mark it was opened from, in order, when it was
    /// and they are all kept.
    pub replay: Option<Vec<Arc<Marked>>>,
    /// Whether the history changed or was cut after that mark.
    pub reset: bool,
    /// What is under way now, when there was no replay: its state up to
    /// `at`, which `events` goes on from.
    pub snapshots: Vec<Arc<Marked>>,
    /// The mark of the last event told before `events`.
    pub at: Mark,
    pub events: Events,
}
fn shard(id: &ResidentId) -> usize {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hash);
    hash.finish() as usize % SHARDS
}

impl Watchers {
    fn get(&self, id: &ResidentId) -> Option<Arc<Mutex<Watching>>> {
        self.watching[shard(id)].read().expect("never poisoned").get(id).cloned()
    }
    fn session(&self, id: &ResidentId) -> Arc<Mutex<Watching>> {
        self.get(id).unwrap_or_else(|| {
            self.watching[shard(id)].write().expect("never poisoned").entry(id.clone()).or_default().clone()
        })
    }
    pub(crate) fn watched(&self, id: &ResidentId) -> bool {
        let watched = self.get(id).is_some_and(|session| {
            session.lock().expect("never poisoned").senders.iter().any(|sender| !sender.is_closed())
        });
        retire(&self.watching, id);
        watched
    }
    pub fn tools(&self, id: &ResidentId) -> Vec<fastctx_types::town::CurrentTool> {
        use fastctx_types::session::Happened;
        let Some(session) = self.get(id) else { return Vec::new() };
        let tools = session
            .lock()
            .expect("never poisoned")
            .live
            .snapshots()
            .filter_map(|event| {
                let Happened::Began { turn, item } = &event.happened else { return None };
                fastctx_types::town::CurrentTool::from_item(turn, item)
            })
            .collect();
        drop(session);
        retire(&self.watching, id);
        tools
    }
    pub async fn unwatched(&self, id: &ResidentId) {
        loop {
            let sender = self.get(id).and_then(|session| {
                session.lock().expect("never poisoned").senders.iter().find(|sender| !sender.is_closed()).cloned()
            });
            retire(&self.watching, id);
            match sender {
                Some(sender) => sender.closed().await,
                None => return,
            }
        }
    }
    pub fn watch(&self, id: &ResidentId) -> Events {
        self.subscribe(id, false)
    }
    pub fn watch_composer(&self, id: &ResidentId) -> Events {
        self.subscribe(id, true)
    }

    /// Hears what is told of `id`'s conversation without viewing it: no
    /// host is kept awake for it, and nothing is kept for it to come back to.
    pub(crate) fn watch_quietly(&self, id: &ResidentId) -> Events {
        self.open(id, false, None, Opened::Quiet).events
    }

    fn subscribe(&self, id: &ResidentId, composer: bool) -> Events {
        self.open(id, composer, None, Opened::Inline).events
    }

    /// Watches `id`'s conversation for a viewer, saying where each event it
    /// is told stands: from `since` on, when every event after it is kept,
    /// those events first and no snapshots; otherwise the snapshots of what
    /// is under way, then each event as it is told.
    pub(crate) fn follow(&self, id: &ResidentId, since: Option<Mark>) -> Followed {
        self.open(id, false, since, Opened::Apart)
    }

    fn open(&self, id: &ResidentId, composer: bool, since: Option<Mark>, how: Opened) -> Followed {
        let (sender, receiver) = events();
        let cell = self.session(id);
        let mut session = cell.lock().expect("never poisoned");
        let replay = since.and_then(|since| session.told.after(since));
        let reset = since.is_some_and(|since| session.told.reset > since.sequence);
        let mut snapshots = Vec::new();
        if replay.is_none() {
            for snapshot in session.live.snapshots() {
                if composer
                    && matches!(
                        &snapshot.happened,
                        fastctx_types::session::Happened::Changed {
                            change: fastctx_types::session::SessionChange::Composer { .. },
                            ..
                        }
                    )
                {
                    continue;
                }
                let snapshot = Arc::new(Marked { mark: None, event: snapshot.clone() });
                if how == Opened::Apart {
                    snapshots.push(snapshot);
                } else if sender.send(snapshot).is_err() {
                    break;
                }
            }
        }
        let quiet = how == Opened::Quiet;
        if quiet {
            session.quiet.push(sender.clone());
        } else {
            session.senders.push(sender.clone());
            session.told.left = None;
        }
        let at = Mark { epoch: session.told.epoch, sequence: session.told.sequence };
        drop(session);
        self.changed.tell(id);
        let changed = self.changed.clone();
        let (registry, id, kept) = (Arc::downgrade(&self.watching), id.clone(), self.kept.clone());
        fastctx_platform::task::spawn(async move {
            sender.closed().await;
            changed.tell(&id);
            let left = {
                let mut session = cell.lock().expect("never poisoned");
                session.quiet.retain(|held| !held.same_channel(&sender));
                session.senders.retain(|held| !held.same_channel(&sender));
                let left = !quiet && session.senders.is_empty();
                if left {
                    session.live.unwatched();
                    session.told.left = Some(Instant::now());
                }
                left
            };
            // What it told is kept a while for a viewer that comes back.
            if left {
                tokio::time::sleep(KEPT_FOR).await;
                let mut session = cell.lock().expect("never poisoned");
                if session.senders.is_empty() && session.told.left.is_some_and(|at| at.elapsed() >= KEPT_FOR) {
                    session.told.forget(&kept);
                }
            }
            drop(cell);
            if let Some(registry) = registry.upgrade() {
                retire(&registry, &id);
            }
        });
        Followed { replay, reset, snapshots, at, events: receiver }
    }

    pub fn publish(&self, id: &ResidentId, event: &SessionEvent) {
        let cell = self.session(id);
        let measured = watch_queue::event_size(event);
        let empty = {
            let mut session = cell.lock().expect("never poisoned");
            if session.live.observe(event) {
                // A conversation no viewer watches, nor left a moment ago,
                // keeps nothing of what it tells.
                let keep = !session.senders.is_empty() || session.told.left.is_some_and(|at| at.elapsed() < KEPT_FOR);
                let marked = session.told.tell(event, measured.0, keep, &self.kept);
                let watching = &mut *session;
                for senders in [&mut watching.senders, &mut watching.quiet] {
                    senders
                        .retain(|sender| sender.send_measured(marked.clone(), measured.0, measured.1.clone()).is_ok());
                }
            }
            if session.senders.is_empty() {
                session.live.unwatched();
            }
            session.senders.is_empty()
                && session.quiet.is_empty()
                && session.live.snapshots().next().is_none()
                && session.told.kept.is_empty()
        };
        drop(cell);
        if empty {
            retire(&self.watching, id);
        }
    }
}

fn retire(registry: &[RwLock<Sessions>; SHARDS], id: &ResidentId) {
    // Callers release their cell before checking. The last concurrent user
    // therefore sees the registry's sole reference, even when several close
    // together. Holding the shard's write lock prevents a new lookup racing it.
    let mut shard = registry[shard(id)].write().expect("never poisoned");
    let empty = shard.get(id).is_some_and(|cell| {
        if Arc::strong_count(cell) != 1 {
            return false;
        }
        let session = cell.lock().expect("never poisoned");
        session.senders.is_empty()
            && session.quiet.is_empty()
            && session.live.snapshots().next().is_none()
            && session.told.kept.is_empty()
    });
    if empty {
        shard.remove(id);
    }
}
