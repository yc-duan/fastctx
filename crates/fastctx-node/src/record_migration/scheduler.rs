//! Bounded background migration jobs, promoted by demand. Waiting callers may
//! leave; owned jobs continue and durable batch checkpoints survive a restart.
mod ready;
use fastctx_platform::{readings, roles};
use fastctx_storage::{Priority, WeakDatabase};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, LazyLock, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const QUEUED: usize = 128;
/// The places in the queue that families no one waits for may take; the
/// rest are kept for families a request waits for.
const BACKGROUND: usize = QUEUED - 32;
const WAIT: Duration = Duration::from_secs(10);
type Key = (String, String);
type Outcome = std::result::Result<(), (io::ErrorKind, String)>;
type Work = Box<dyn FnOnce() -> io::Result<()> + Send>;
static ENGINES: LazyLock<Mutex<HashMap<PathBuf, Weak<Engine>>>> = LazyLock::new(Default::default);
static DEPTH: LazyLock<&'static readings::Gauge> = LazyLock::new(|| readings::gauge("migration.queued"));
static ACTIVE: LazyLock<&'static readings::Gauge> = LazyLock::new(|| readings::gauge("migration.active"));
static BUSY: LazyLock<&'static readings::Counter> = LazyLock::new(|| readings::counter("migration.busy"));
static ELAPSED: LazyLock<&'static readings::Timing> = LazyLock::new(|| readings::timing("migration.family"));
static FAILED: LazyLock<&'static readings::Gauge> = LazyLock::new(|| readings::gauge("migration.failed_families"));
thread_local! {
    static CURRENT: RefCell<Option<(Weak<Engine>, Arc<Cell>)>> = const { RefCell::new(None) };
    static DISCOVERING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct Cell {
    urgent: AtomicBool,
    cancelled: AtomicBool,
    result: Mutex<Option<Outcome>>,
    changed: Condvar,
}
struct Job {
    key: Key,
    cell: Arc<Cell>,
    work: Work,
}
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    pending: HashMap<Key, Weak<Cell>>,
    ready: ready::Ready,
    failed: HashMap<Key, Failed>,
    /// Conversations this process created: they hold no earlier records.
    born: HashSet<String>,
}
/// A family that failed for a reason other than being busy or stopped waits
/// before it runs again, so a lasting fault neither spins a worker nor fills
/// the log; its reason is told once, and again only when it changes.
struct Failed {
    attempts: u32,
    retry_at: Instant,
    reason: String,
}
struct Engine {
    database: WeakDatabase,
    queue: Mutex<Queue>,
    changed: Condvar,
    started: AtomicBool,
    opened: tokio::sync::Notify,
    progress: tokio::sync::Notify,
    closed: AtomicBool,
    log: std::sync::OnceLock<Arc<crate::log::Log>>,
}

/// Owned by the node, independently of the jobs that its workers are draining.
pub(crate) struct Service {
    engine: Arc<Engine>,
}
impl Drop for Service {
    fn drop(&mut self) {
        self.engine.close();
    }
}
impl Service {
    pub(crate) fn register(world: &Path, database: &fastctx_storage::Database) -> Self {
        let engine = Arc::new(Engine {
            database: database.downgrade(),
            queue: Mutex::new(Queue::default()),
            changed: Condvar::new(),
            opened: tokio::sync::Notify::new(),
            progress: tokio::sync::Notify::new(),
            started: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            log: std::sync::OnceLock::new(),
        });
        let mut engines = ENGINES.lock().expect("migration registry");
        engines.retain(|_, engine| engine.strong_count() > 0);
        engines.insert(world.to_path_buf(), Arc::downgrade(&engine));
        Self { engine }
    }

    /// Where a family's failure is told; without it, failures only wait to retry.
    pub(crate) fn report_to(&self, log: Arc<crate::log::Log>) {
        let _ = self.engine.log.set(log);
    }

    /// Called only after the listener has been polled, never during startup IO.
    pub(crate) fn start(&self) -> io::Result<bool> {
        if self.engine.started.swap(true, Ordering::AcqRel) {
            return Ok(false);
        }
        // Families no one waits for move on as many workers as the machine
        // has cores, up to two: a single one waits on the database writer and
        // on small files most of the time. One more worker takes only the
        // families asked for now, so a request never waits behind them.
        let background = std::thread::available_parallelism().map_or(1, |cores| cores.get().clamp(1, 2));
        for index in 0..=background {
            let engine = self.engine.clone();
            if let Err(error) = std::thread::Builder::new()
                .name(format!("record-migration-{index}"))
                .spawn(move || engine.run(index < background))
            {
                self.engine.close();
                return Err(error);
            }
        }
        self.engine.opened.notify_waiters();
        Ok(true)
    }
    pub(crate) async fn wait_progress(&self) {
        self.engine.progress.notified().await;
    }

    pub(crate) async fn wait_started(&self) {
        loop {
            let opened = self.engine.opened.notified();
            tokio::pin!(opened);
            opened.as_mut().enable();
            if self.engine.started.load(Ordering::Acquire) {
                return;
            }
            opened.await;
        }
    }
}

fn busy(message: &str) -> io::Error {
    BUSY.add(1);
    io::Error::new(io::ErrorKind::WouldBlock, format!("record migration is busy: {message}; retry with backoff"))
}

pub(crate) fn discovery<T>(work: impl FnOnce() -> T) -> T {
    let previous = DISCOVERING.replace(true);
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            DISCOVERING.set(self.0);
        }
    }
    let _reset = Reset(previous);
    work()
}

pub(crate) fn priority() -> Priority {
    CURRENT.with_borrow(|current| match current {
        Some((_, cell)) if cell.urgent.load(Ordering::Acquire) => Priority::Interactive,
        _ => Priority::Background,
    })
}

/// A batch cooperates with shutdown without treating cancellation as completion.
pub(crate) fn checkpoint() -> io::Result<()> {
    CURRENT.with_borrow(|current| {
        if current.as_ref().is_some_and(|(engine, cell)| {
            cell.cancelled.load(Ordering::Acquire)
                || engine.upgrade().is_none_or(|engine| engine.closed.load(Ordering::Acquire))
        }) {
            Err(io::Error::new(io::ErrorKind::Interrupted, "migration is stopping at its batch boundary"))
        } else {
            Ok(())
        }
    })
}

/// Whether discovery may ask for more families now: the queue has room
/// for those no one waits for.
pub(crate) fn admits(world: &Path) -> bool {
    engine(world).is_some_and(|engine| engine.queue.lock().expect("migration queue").jobs.len() < BACKGROUND)
}

/// How few families wait in the queue before discovery asks for more.
pub(crate) const DRAINED: usize = BACKGROUND / 2;

/// How many families wait in the queue now.
pub(crate) fn queued(world: &Path) -> usize {
    engine(world).map_or(0, |engine| engine.queue.lock().expect("migration queue").jobs.len())
}

/// Whether a family of `conversation` is still queued or moving.
pub(crate) fn moving(world: &Path, conversation: &str) -> bool {
    engine(world).is_some_and(|engine| {
        let queue = engine.queue.lock().expect("migration queue");
        queue.pending.iter().any(|(key, cell)| key.0 == conversation && cell.strong_count() > 0)
    })
}

fn engine(world: &Path) -> Option<Arc<Engine>> {
    ENGINES.lock().expect("migration registry").get(world).and_then(Weak::upgrade)
}

/// The generation a conversation this release creates is kept as born at
/// (`born`). Raise it whenever a migration is added for records this release
/// writes: the conversations born before then are looked at again.
pub(crate) const BORN: i64 = 1;

/// A conversation this release creates has no earlier records, so none of
/// its families waits for a migration, on any thread. It is kept as born
/// (`record_migration::born`) until a gather or an import changes what it
/// holds.
pub(crate) fn born(world: &Path, conversation: &str) {
    if let Some(engine) = ENGINES.lock().expect("migration registry").get(world).and_then(Weak::upgrade) {
        engine.queue.lock().expect("migration queue").born.insert(conversation.to_string());
    }
}

/// Whether `family` of `conversation` is known to have moved, without a
/// read of its marker.
pub(crate) fn known(world: &Path, conversation: &str, family: &str) -> bool {
    let Some(engine) = ENGINES.lock().expect("migration registry").get(world).and_then(Weak::upgrade) else {
        return false;
    };
    let queue = engine.queue.lock().expect("migration queue");
    queue.ready.has(conversation, family) || queue.born.contains(conversation)
}

/// A blocking reader has already checked the durable completion marker.
pub(crate) fn ready(world: &Path, conversation: &str, family: &str) {
    if let Some(engine) = ENGINES.lock().expect("migration registry").get(world).and_then(Weak::upgrade) {
        engine.remember((conversation.to_string(), family.to_string()));
    }
}

/// Loads the completed gathered layouts while registration is still on a
/// blocking thread.
pub(crate) fn hydrate(
    world: &Path,
    database: &fastctx_storage::Database,
    excluded: &HashSet<String>,
    scope: Option<&str>,
) -> io::Result<()> {
    let ready = database.read_background_now(|db| ready::load(db, excluded, scope)).map_err(io::Error::other)?;
    // Those born here before the node last stopped, as the node opens.
    let born = match scope {
        None => database.read_background_now(|db| ready::born(db, excluded)).map_err(io::Error::other)?,
        Some(_) => Vec::new(),
    };
    if let Some(engine) = ENGINES.lock().expect("migration registry").get(world).and_then(Weak::upgrade) {
        let mut queue = engine.queue.lock().expect("migration queue");
        queue.ready.extend(ready);
        queue.born.extend(born);
    }
    Ok(())
}

/// Discards admission facts before a gather, transfer or deletion changes scope.
pub(crate) fn invalidate(world: &Path, conversation: &str) {
    let engine = ENGINES.lock().expect("migration registry").get(world).and_then(Weak::upgrade);
    let Some(engine) = engine else {
        return;
    };
    let mut queue = engine.queue.lock().expect("migration queue");
    queue.ready.delete(conversation);
    queue.born.remove(conversation);
    queue.failed.retain(|key, _| key.0 != conversation);
    FAILED.set(queue.failed.len() as i64);
    let current = CURRENT.with_borrow(|current| current.as_ref().map(|(_, cell)| Arc::downgrade(cell)));
    queue.pending.retain(|key, cell| {
        if key.0 != conversation || current.as_ref().is_some_and(|current| current.ptr_eq(cell)) {
            return true;
        }
        if let Some(cell) = cell.upgrade() {
            cell.cancelled.store(true, Ordering::Release);
        }
        false
    });
    queue.jobs.retain(|job| {
        if job.key.0 != conversation {
            return true;
        }
        *job.cell.result.lock().expect("migration result") =
            Some(Err((io::ErrorKind::Interrupted, "migration generation changed".into())));
        job.cell.changed.notify_all();
        DEPTH.add(-1);
        false
    });
    engine.progress.notify_one();
}

/// Asks for `family` of `conversation` to move, as `request` does from a
/// request, without waiting: whether it has moved. One that has not moves
/// on a worker meanwhile; a task asks again as the migration progresses
/// (`Service::wait_progress`), holding no thread while it waits.
pub(crate) fn ask(
    world: &Path,
    conversation: &str,
    family: &str,
    work: impl FnOnce() -> io::Result<()> + Send + 'static,
) -> io::Result<bool> {
    let engine = ENGINES
        .lock()
        .expect("migration registry")
        .get(world)
        .and_then(Weak::upgrade)
        .ok_or_else(|| busy("the node has not opened migration yet"))?;
    let key = (conversation.to_string(), family.to_string());
    let mut queue = engine.queue.lock().expect("migration queue");
    if queue.ready.contains(&key) || queue.born.contains(&key.0) {
        return Ok(true);
    }
    if engine.closed.load(Ordering::Acquire) {
        return Err(busy("the node is stopping"));
    }
    if let Some(cell) = queue.pending.get(&key).and_then(Weak::upgrade) {
        cell.urgent.store(true, Ordering::Release);
        engine.changed.notify_all();
        return Ok(false);
    }
    if let Some(failed) = queue.failed.get(&key)
        && failed.retry_at > Instant::now()
    {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("migrating {} failed and is tried again later: {}", key.1, failed.reason),
        ));
    }
    if !engine.started.load(Ordering::Acquire) {
        return Err(busy("the listener is still opening"));
    }
    if queue.jobs.len() >= QUEUED {
        return Err(busy("the migration queue is full"));
    }
    let cell = Arc::new(Cell {
        urgent: AtomicBool::new(true),
        cancelled: AtomicBool::new(false),
        result: Mutex::new(None),
        changed: Condvar::new(),
    });
    queue.pending.insert(key.clone(), Arc::downgrade(&cell));
    queue.jobs.push_back(Job { key, cell, work: Box::new(work) });
    DEPTH.add(1);
    engine.changed.notify_all();
    Ok(false)
}

pub(crate) fn request(
    world: &Path,
    conversation: &str,
    family: &str,
    work: impl FnOnce() -> io::Result<()> + Send + 'static,
) -> io::Result<()> {
    let began = Instant::now();
    let engine = ENGINES
        .lock()
        .expect("migration registry")
        .get(world)
        .and_then(Weak::upgrade)
        .ok_or_else(|| busy("the node has not opened migration yet"))?;
    let key = (conversation.to_string(), family.to_string());
    let urgent = CURRENT.with_borrow(|current| {
        current.as_ref().map_or_else(|| !DISCOVERING.get(), |(_, cell)| cell.urgent.load(Ordering::Acquire))
    });
    // Derived migrations can prepare prerequisites on their existing worker,
    // without waiting for the same bounded pool to supply another worker.
    let nested = CURRENT.with_borrow(|current| current.is_some());
    let cell = {
        let mut queue = engine.queue.lock().expect("migration queue");
        if queue.ready.contains(&key) || queue.born.contains(&key.0) {
            return Ok(());
        }
        if engine.closed.load(Ordering::Acquire) {
            return Err(busy("the node is stopping"));
        }
        if let Some(cell) = queue.pending.get(&key).and_then(Weak::upgrade) {
            if urgent {
                cell.urgent.store(true, Ordering::Release);
                engine.changed.notify_all();
            }
            if nested && let Some(index) = queue.jobs.iter().position(|job| job.key == key) {
                let job = queue.jobs.remove(index).expect("queued prerequisite");
                DEPTH.add(-1);
                drop(queue);
                return engine.perform(job);
            }
            if nested {
                return Err(busy("a prerequisite is already running"));
            }
            cell
        } else {
            if let Some(failed) = queue.failed.get(&key)
                && failed.retry_at > Instant::now()
            {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("migrating {} failed and is tried again later: {}", key.1, failed.reason),
                ));
            }
            if nested {
                let cell = Arc::new(Cell {
                    urgent: AtomicBool::new(urgent),
                    cancelled: AtomicBool::new(false),
                    result: Mutex::new(None),
                    changed: Condvar::new(),
                });
                queue.pending.insert(key.clone(), Arc::downgrade(&cell));
                drop(queue);
                return engine.perform(Job { key, cell, work: Box::new(work) });
            }
            if !engine.started.load(Ordering::Acquire) {
                return Err(busy("the listener is still opening"));
            }
            if queue.jobs.len() >= if urgent { QUEUED } else { BACKGROUND } {
                return Err(busy("the migration queue is full"));
            }
            let cell = Arc::new(Cell {
                urgent: AtomicBool::new(urgent),
                cancelled: AtomicBool::new(false),
                result: Mutex::new(None),
                changed: Condvar::new(),
            });
            queue.pending.insert(key.clone(), Arc::downgrade(&cell));
            queue.jobs.push_back(Job { key, cell: cell.clone(), work: Box::new(work) });
            DEPTH.add(1);
            engine.changed.notify_all();
            cell
        }
    };
    if DISCOVERING.get() {
        return Err(io::Error::new(io::ErrorKind::WouldBlock, "legacy migration is queued"));
    }
    // A thread that keeps network lines alive never waits for records.
    if matches!(roles::current(), roles::Role::Link | roles::Role::Data) {
        return Err(busy("its records are waiting to move"));
    }
    roles::in_place("waiting for records to move", || {
        let mut result = cell.result.lock().expect("migration result");
        loop {
            if let Some(result) = &*result {
                return result.clone().map_err(|(kind, message)| io::Error::new(kind, message));
            }
            let Some(remaining) = WAIT.checked_sub(began.elapsed()) else {
                return Err(busy("the ten-second wait expired"));
            };
            result = cell.changed.wait_timeout(result, remaining).expect("migration result").0;
        }
    })
}

impl Engine {
    fn remember(&self, key: Key) {
        self.queue.lock().expect("migration queue").ready.insert(key);
    }
    fn failed(&self, queue: &mut Queue, key: &Key, reason: String) {
        let failed = queue.failed.entry(key.clone()).or_insert(Failed {
            attempts: 0,
            retry_at: Instant::now(),
            reason: String::new(),
        });
        failed.attempts = failed.attempts.saturating_add(1);
        let wait = Duration::from_secs(1u64 << failed.attempts.min(12)).min(Duration::from_secs(60 * 60));
        failed.retry_at = Instant::now() + wait;
        if failed.reason != reason {
            if let Some(log) = self.log.get() {
                log.line(format_args!(
                    "migrating {} of {} failed; trying again in {}s: {reason}",
                    key.1,
                    key.0,
                    wait.as_secs()
                ));
            }
            failed.reason = reason;
        }
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let jobs = {
            let mut queue = self.queue.lock().expect("migration queue");
            let jobs = std::mem::take(&mut queue.jobs);
            DEPTH.add(-(jobs.len() as i64));
            jobs
        };
        for job in jobs {
            *job.cell.result.lock().expect("migration result") =
                Some(Err((io::ErrorKind::Interrupted, "migration stopped before this family".into())));
            job.cell.changed.notify_all();
        }
        self.changed.notify_all();
    }
    fn run(self: Arc<Self>, background: bool) {
        let _role = roles::scoped(roles::Role::Blocking);
        let mut urgent = 0;
        loop {
            let job = {
                let mut queue = self.queue.lock().expect("migration queue");
                while (queue.jobs.is_empty()
                    || (!background && !queue.jobs.iter().any(|job| job.cell.urgent.load(Ordering::Acquire))))
                    && !self.closed.load(Ordering::Acquire)
                {
                    queue = self.changed.wait(queue).expect("migration queue");
                }
                if self.closed.load(Ordering::Acquire) || self.database.upgrade().is_none() {
                    return;
                }
                let wanted = !background || urgent < 8;
                let index =
                    queue.jobs.iter().position(|job| job.cell.urgent.load(Ordering::Acquire) == wanted).unwrap_or(0);
                let job = queue.jobs.remove(index).expect("nonempty migration queue");
                if job.cell.urgent.load(Ordering::Acquire) {
                    urgent += 1;
                } else {
                    urgent = 0;
                }
                DEPTH.add(-1);
                job
            };
            let _ = self.perform(job);
        }
    }
    fn perform(self: &Arc<Self>, job: Job) -> io::Result<()> {
        let started = Instant::now();
        ACTIVE.add(1);
        let previous = CURRENT.replace(Some((Arc::downgrade(self), job.cell.clone())));
        let mut result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            checkpoint()?;
            (job.work)()
        }))
        .unwrap_or_else(|_| Err(io::Error::other("migration worker panicked")));
        CURRENT.set(previous);
        ACTIVE.add(-1);
        ELAPSED.record(started.elapsed());
        {
            let mut queue = self.queue.lock().expect("migration queue");
            if result.is_ok() {
                if job.cell.cancelled.load(Ordering::Acquire) {
                    result = Err(io::Error::new(io::ErrorKind::Interrupted, "migration generation changed"));
                } else {
                    queue.ready.insert(job.key.clone());
                    queue.failed.remove(&job.key);
                }
            }
            if let Err(error) = &result
                && !matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
            {
                self.failed(&mut queue, &job.key, error.to_string());
            }
            FAILED.set(queue.failed.len() as i64);
            if queue.pending.get(&job.key).is_some_and(|cell| cell.ptr_eq(&Arc::downgrade(&job.cell))) {
                queue.pending.remove(&job.key);
            }
        }
        // Every task waiting for progress asks again, and the next to wait
        // finds it too.
        self.progress.notify_waiters();
        self.progress.notify_one();
        *job.cell.result.lock().expect("migration result") =
            Some(result.as_ref().map(|_| ()).map_err(|error| (error.kind(), error.to_string())));
        job.cell.changed.notify_all();
        result
    }
}
