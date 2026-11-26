//! The durable queue of one service's work that takes time.
//!
//! A call that asks for such work does not do it: in the transaction that
//! records what was asked it submits a job, and is answered. The job is a
//! row of the service's own database, so it is there or not with what asked
//! for it, and it is run after: a few of a kind at once, one of a unit at a
//! time, again after a failure with a growing wait, and set aside when it
//! keeps failing. Asking again for work already waiting is the same job
//! with what was asked last; asking while it runs has it run once more.
//!
//! Jobs are not answers. What a job did is in the state it changed, which
//! whoever asked reads or is told of.

mod measures;
mod operations;
mod run;
pub mod submitted;
mod table;

pub use operations::{Operation, RESULT_BYTES, Standing, accept, settle, standing};
pub use table::{
    KEY_BYTES, PAYLOAD_BYTES, SCHEMA, forget, rouse, submit, submit_after, submit_unless_waiting, submit_within,
    withdraw,
};

use fastctx_storage::Database;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

/// Every runner of a kind in this process, by the kind's name: submitting a
/// job tells them at once (`stirred`), so a runner with nothing due sleeps
/// until then, or until the earliest job it holds falls due, and never looks
/// at its table only in case (`run::IDLE` is a long safety net).
static RUNNERS: LazyLock<Mutex<Runners>> = LazyLock::new(Default::default);

/// The runners of each kind, by the kind's name (`RUNNERS`).
type Runners = HashMap<&'static str, Vec<Weak<Registered>>>;

/// Tells every runner in this process to look at its table again: after the
/// machine slept or its clock jumped, a job due by the wall clock may be due
/// though the runner's sleep has not ended.
pub fn stir_all() {
    let runners: Vec<Arc<Registered>> = RUNNERS
        .lock()
        .expect("job runners")
        .values()
        .flat_map(|runners| runners.iter().filter_map(Weak::upgrade))
        .collect();
    for registered in runners {
        registered.stirred.notify_one();
    }
}

/// Tells the runners of `kind` that a job of it may have become due. Called
/// in the transaction that submits it: a runner's next look goes through the
/// same database's writer, after that transaction, so it sees the job.
pub(crate) fn stirred(kind: &str) {
    let runners: Vec<Arc<Registered>> = RUNNERS
        .lock()
        .expect("job runners")
        .get(kind)
        .map(|runners| runners.iter().filter_map(Weak::upgrade).collect())
        .unwrap_or_default();
    for registered in runners {
        registered.stirred.notify_one();
    }
}

/// One kind of work and how it is run on this machine.
#[derive(Clone, Copy, Debug)]
pub struct Kind {
    pub name: &'static str,
    /// Jobs of the kind run at once, at most.
    pub at_once: usize,
    /// How long one run may take; past it the run is dropped and counts as failed.
    pub within: Duration,
    /// Failures in a row after which a job is set aside until it is asked for again.
    pub tries: u32,
    /// How long until a job of the kind with this payload may begin, beyond
    /// its own due time: what the kind paces all its jobs by. It is looked
    /// at before a job is taken, so one that may not begin yet is not taken
    /// only to be put back, a write each way (codex-7d36e00c). None for a
    /// kind that paces nothing.
    pub ready: Option<Ready>,
}

/// How long until a job with this payload may begin (`Kind::ready`).
pub type Ready = fn(&[u8]) -> Option<Duration>;

/// Work to be done: of a kind, named so asking again finds it, for a unit
/// (a conversation, an account) whose jobs of a kind run one at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub kind: &'static str,
    pub key: String,
    pub unit: String,
    pub payload: Vec<u8>,
}

/// A job as its run receives it.
#[derive(Clone, Debug)]
pub struct Taken {
    pub key: String,
    pub unit: String,
    pub payload: Vec<u8>,
    /// Which try this is, from one.
    pub attempt: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The work is done and the job is no more.
    Done,
    /// There is more of it to do, not before `after` has passed.
    Again { after: Duration },
    /// It did not get done, and why; it is tried again after a wait.
    Failed(String),
}

/// What runs one kind's jobs.
pub type Run = Arc<dyn Fn(Taken) -> Pin<Box<dyn Future<Output = Outcome> + Send>> + Send + Sync>;

/// The jobs of one database and what runs them.
pub struct Jobs {
    database: Database,
    kinds: Mutex<HashMap<&'static str, Arc<Registered>>>,
    started: AtomicBool,
    /// The runtime the jobs run on, once they are started.
    runtime: std::sync::OnceLock<tokio::runtime::Handle>,
    /// The operations someone waits for the settling of.
    waited: Mutex<HashMap<Operation, tokio::sync::watch::Sender<u64>>>,
}

struct Registered {
    kind: Kind,
    run: Run,
    /// Told when a job of the kind may have become due or a unit free.
    stirred: tokio::sync::Notify,
    /// The units a job of the kind runs for now.
    busy: Mutex<HashSet<String>>,
    measures: measures::Measures,
    /// Whether the kind's runner was started: it is started once, by
    /// whichever of `register` and `start` comes second.
    served: AtomicBool,
}

impl Jobs {
    /// The jobs kept in `database`, whose schema includes `SCHEMA`.
    pub fn new(database: Database) -> Arc<Self> {
        Arc::new(Self {
            database,
            kinds: Mutex::default(),
            started: AtomicBool::new(false),
            runtime: std::sync::OnceLock::new(),
            waited: Mutex::default(),
        })
    }

    /// Names what runs a kind's jobs. A kind registered before `start` runs
    /// from the start; one registered after runs from then, on the runtime
    /// the jobs were started on.
    pub fn register(&self, kind: Kind, run: Run) {
        assert!(kind.at_once > 0 && kind.tries > 0, "a kind runs at least one job, at least once");
        let registered = Arc::new(Registered {
            kind,
            run,
            stirred: tokio::sync::Notify::new(),
            busy: Mutex::default(),
            measures: measures::Measures::of(kind.name),
            served: AtomicBool::new(false),
        });
        let known = self.kinds.lock().expect("job kinds").insert(kind.name, registered.clone());
        assert!(known.is_none(), "the job kind {} is registered twice", kind.name);
        {
            let mut runners = RUNNERS.lock().expect("job runners");
            let of_kind = runners.entry(kind.name).or_default();
            of_kind.retain(|runner| runner.strong_count() > 0);
            of_kind.push(Arc::downgrade(&registered));
        }
        if let Some(runtime) = self.runtime.get() {
            run::serve_once(runtime, self.database.clone(), registered);
        }
    }

    /// The database the jobs are kept in.
    pub fn database(&self) -> &Database {
        &self.database
    }

    /// Tells the kind's runner that a job was submitted and committed.
    pub fn stir(&self, kind: &str) {
        if let Some(registered) = self.kinds.lock().expect("job kinds").get(kind) {
            registered.stirred.notify_one();
        }
    }
}
