//! What this process's tasks are doing and what each waits for
//! (`specs/world-concurrency.md`).
//!
//! The locks that guard more than a leaf of bookkeeping are made here: tokio's
//! for tasks, and `SyncMutex` for the short work on the node's own records
//! that blocks its thread. Each knows its place in the order locks are taken
//! in (`Rank`); a task or thread that takes one while holding one of the same
//! or a later place is told about (`set_reporter`), as that is how two of
//! them come to wait on each other.
//! Every such lock held or waited for, and every wait on something outside
//! the process (`outside`), is kept in one registry with what its task is
//! doing (`doing`, `labeled`), so the process can say at any moment who waits for
//! what, who holds it and since when, and find tasks that wait on each other
//! in a circle (`snapshot`).

mod lock;
mod registry;
mod sync;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::Duration;

pub use lock::{
    Busy, Mutex, MutexGuard, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, RwLockReadGuard,
    RwLockWriteGuard,
};
pub use registry::{Active, Cycle, OutsideWait, Snapshot, Stuck, Work, progress, set_reporter, snapshot, violations};
pub use sync::{SyncMutex, SyncMutexGuard};

/// A lock's place in the order locks are taken in: a task holding a lock
/// takes only locks of later places, or of the same place with a later
/// subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rank(u16);

impl Rank {
    // Locks that let a whole round of work run once at a time: taken before
    // any conversation's, and never while holding one.
    /// One drain of the deliveries this machine owes other machines.
    pub const DRAINING: Rank = Rank(10);
    /// Sending the deletions asked of other machines.
    pub const FORGETTING: Rank = Rank(12);
    /// Persisting incoming letters of one hub across reconnects.
    pub const RECEIVING: Rank = Rank(11);
    /// Scanning or changing the agents' logins.
    pub const LOGINS: Rank = Rank(14);
    /// Carrying an account's collaboration files.
    pub const CARRYING: Rank = Rank(16);
    /// Claiming a command's end, once.
    pub const JOB_END: Rank = Rank(20);
    /// Writing one copy's destination.
    pub const COPYING: Rank = Rank(30);
    /// Claude sessions being born and claimed: a launch holds it shared
    /// until it knows its session, a registration alone while it is placed.
    pub const BIRTHS: Rank = Rank(95);

    // A conversation's locks, in the order they are taken.
    /// Its place on this machine: deleting it holds it alone; starting a
    /// command for it, reading a command's output there, receiving a file
    /// for it and a session claiming its directory share it.
    pub const PLACE: Rank = Rank(90);
    /// User actions enter one conversation in order, before lifecycle or host locks.
    pub const ACTION: Rank = Rank(85);
    /// Its management gate: what changes its life holds it alone, what
    /// uses it shares it.
    pub const LIFECYCLE: Rank = Rank(100);
    /// The reports of one of its Claude sessions.
    pub const SESSION: Rank = Rank(200);
    /// The reports of its Codex thread.
    pub const THREAD: Rank = Rank(210);
    /// Handing its words to its Claude session.
    pub const DELIVERING: Rank = Rank(250);
    /// Its words and turns.
    pub const SPEAKING: Rank = Rank(300);
    /// Paying out its answers.
    pub const PAYING: Rank = Rank(320);
    /// Its shared input.
    pub const COMPOSER: Rank = Rank(400);

    // Bookkeeping over many conversations, after every conversation's lock.
    /// One login's listing of Claude Code's background sessions.
    pub const LISTING: Rank = Rank(510);
    /// Decoding a picture into a thumbnail.
    pub const DECODING: Rank = Rank(520);
    /// Reading an attached picture's record.
    pub const PICTURES: Rank = Rank(530);
    /// Loading a thread into one login's Codex app-server.
    pub const CODEX_LOADING: Rank = Rank(600);
    /// Starting one login's Codex app-server.
    pub const CODEX_STARTING: Rank = Rank(610);
    /// Writing a project's or a page's records.
    pub const WRITING: Rank = Rank(700);
    /// Syncing the ledger with the network.
    pub const SYNCING: Rank = Rank(710);

    // The node's own records, after every lock above: a thread holding one
    // reads and writes a few of its files and waits on nothing outside
    // (`SyncMutex`).
    /// Jobs' records kept from collection while they are made, read or
    /// changed.
    pub const DISK: Rank = Rank(800);
    /// A command's call facts, emitted in order.
    pub const EMITTING: Rank = Rank(810);
    /// Conversations' records, each read and written as one transaction.
    pub const RECORDS: Rank = Rank(820);
    /// A command's write facts.
    pub const WRITE_FACTS: Rank = Rank(830);
}

/// What each task says it does, latest last, by the token of each saying.
type Labels = HashMap<tokio::task::Id, Vec<(u64, Arc<str>)>>;

static LABELS: LazyLock<StdMutex<Labels>> = LazyLock::new(Default::default);

thread_local! {
    static THREAD: RefCell<Option<Arc<str>>> = const { RefCell::new(None) };
}

/// What a task does from here until this is dropped: the locks it waits for
/// and holds, and its waits outside, say so. Set within one, a later one says
/// it until it is dropped.
pub struct Doing {
    task: Option<tokio::task::Id>,
    token: u64,
}

/// Says what the current task does from here, until the result is dropped.
pub fn doing(label: impl Into<Arc<str>>) -> Doing {
    let task = tokio::task::try_id();
    let token = registry::next();
    if let Some(task) = task {
        let mut labels = LABELS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        labels.entry(task).or_default().push((token, label.into()));
    }
    Doing { task, token }
}

impl Drop for Doing {
    fn drop(&mut self) {
        let Some(task) = self.task else { return };
        let mut labels = LABELS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(said) = labels.get_mut(&task) {
            said.retain(|(token, _)| *token != self.token);
            if said.is_empty() {
                labels.remove(&task);
            }
        }
    }
}

/// Runs `work` as doing `label`.
pub async fn labeled<F: Future>(label: impl Into<Arc<str>>, work: F) -> F::Output {
    let _doing = doing(label);
    work.await
}

/// `work`, to run on a blocking thread as doing what the current task does.
pub fn carried<R>(work: impl FnOnce() -> R) -> impl FnOnce() -> R {
    let label = label();
    move || {
        // The pool's thread goes on to other work, also after a panic.
        struct Said;
        impl Drop for Said {
            fn drop(&mut self) {
                THREAD.with(|said| *said.borrow_mut() = None);
            }
        }
        THREAD.with(|said| *said.borrow_mut() = Some(label));
        let _said = Said;
        work()
    }
}

/// What the current task, or blocking thread, is doing, as it was said. A
/// blocking task runs work `carried` from the task that started it, which
/// says what it does.
pub fn label() -> Arc<str> {
    if let Some(carried) = THREAD.with(|said| said.borrow().clone()) {
        return carried;
    }
    match tokio::task::try_id() {
        Some(task) => {
            let labels = LABELS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            labels
                .get(&task)
                .and_then(|said| said.last())
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| Arc::from(format!("{UNSAID_TASK}{task}")))
        }
        None => Arc::from(UNSAID_THREAD),
    }
}

/// What `label` names a task or thread by when it said nothing it does.
const UNSAID_TASK: &str = "task ";
const UNSAID_THREAD: &str = "a blocking thread";

/// Whether `label` is something a task said it does (`doing`), not only
/// which task it is.
fn said(label: &str) -> bool {
    label != UNSAID_THREAD && !label.strip_prefix(UNSAID_TASK).is_some_and(|id| id.bytes().all(|b| b.is_ascii_digit()))
}

/// Something outside the process did not answer in time.
#[derive(Clone, Debug)]
pub struct Late {
    pub what: String,
    pub within: Duration,
}

impl fmt::Display for Late {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} did not answer within {} s", self.what, self.within.as_secs())
    }
}

impl std::error::Error for Late {}

/// Waits for `work`, something outside the process named by `what`, for at
/// most `within`; kept in the registry while it lasts.
pub async fn outside<F: Future>(what: impl Into<String>, within: Duration, work: F) -> Result<F::Output, Late> {
    let what = what.into();
    let _wait = OutsideWait::begin(what.clone(), within);
    tokio::time::timeout(within, work).await.map_err(|_| Late { what, within })
}

/// Keeps a wait outside the process in the registry until dropped, for code
/// that enforces its own deadline, such as a blocking call that ends a
/// program it started.
pub fn waiting_outside(what: impl Into<String>, within: Duration) -> OutsideWait {
    OutsideWait::begin(what.into(), within)
}
