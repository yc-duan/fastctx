//! Bounded background migration jobs, promoted by demand. Waiting callers may
//! leave; owned jobs continue and durable batch checkpoints survive a restart.
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
const READY: usize = 8192;
const WAIT: Duration = Duration::from_secs(10);
const WORKERS: usize = 2;
type Key = (String, String);
type Outcome = std::result::Result<(), (io::ErrorKind, String)>;
type Work = Box<dyn FnOnce() -> io::Result<()> + Send>;
static ENGINES: LazyLock<Mutex<HashMap<PathBuf, Weak<Engine>>>> = LazyLock::new(Default::default);
static DEPTH: LazyLock<&'static readings::Gauge> = LazyLock::new(|| readings::gauge("migration.queued"));
static ACTIVE: LazyLock<&'static readings::Gauge> = LazyLock::new(|| readings::gauge("migration.active"));
static BUSY: LazyLock<&'static readings::Counter> = LazyLock::new(|| readings::counter("migration.busy"));
static ELAPSED: LazyLock<&'static readings::Timing> = LazyLock::new(|| readings::timing("migration.family"));
thread_local! {
    static CURRENT: RefCell<Option<(Weak<Engine>, Arc<Cell>)>> = const { RefCell::new(None) };
    static DISCOVERING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct Cell {
    urgent: AtomicBool,
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
    ready: HashSet<Key>,
    oldest: VecDeque<Key>,
}
struct Engine {
    database: WeakDatabase,
    queue: Mutex<Queue>,
    changed: Condvar,
    started: AtomicBool,
    opened: tokio::sync::Notify,
    progress: tokio::sync::Notify,
    closed: AtomicBool,
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
        });
        let mut engines = ENGINES.lock().expect("migration registry");
        engines.retain(|_, engine| engine.strong_count() > 0);
        engines.insert(world.to_path_buf(), Arc::downgrade(&engine));
        Self { engine }
    }

    /// Called only after the listener has been polled, never during startup IO.
    pub(crate) fn start(&self) -> io::Result<bool> {
        if self.engine.started.swap(true, Ordering::AcqRel) {
            return Ok(false);
        }
        for index in 0..WORKERS {
            let engine = self.engine.clone();
            if let Err(error) = std::thread::Builder::new()
                .name(format!("record-migration-{index}"))
                .spawn(move || engine.run(index == 0))
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
        if current
            .as_ref()
            .is_some_and(|(engine, _)| engine.upgrade().is_none_or(|engine| engine.closed.load(Ordering::Acquire)))
        {
            Err(io::Error::new(io::ErrorKind::Interrupted, "migration is stopping at its batch boundary"))
        } else {
            Ok(())
        }
    })
}

/// A blocking reader has already checked the durable completion marker.
pub(crate) fn ready(world: &Path, conversation: &str, family: &str) {
    if let Some(engine) = ENGINES.lock().expect("migration registry").get(world).and_then(Weak::upgrade) {
        engine.remember((conversation.to_string(), family.to_string()));
    }
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
        if queue.ready.contains(&key) {
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
            if nested {
                let cell = Arc::new(Cell {
                    urgent: AtomicBool::new(urgent),
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
            if queue.jobs.len() >= if urgent { QUEUED } else { QUEUED - 32 } {
                return Err(busy("the migration queue is full"));
            }
            let cell =
                Arc::new(Cell { urgent: AtomicBool::new(urgent), result: Mutex::new(None), changed: Condvar::new() });
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
    if !roles::current().may_block() {
        return Err(busy("this conversation is being imported"));
    }
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
}

impl Engine {
    fn remember(&self, key: Key) {
        let mut queue = self.queue.lock().expect("migration queue");
        if queue.ready.insert(key.clone()) {
            queue.oldest.push_back(key);
        }
        while queue.oldest.len() > READY {
            if let Some(oldest) = queue.oldest.pop_front() {
                queue.ready.remove(&oldest);
            }
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
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.work))
            .unwrap_or_else(|_| Err(io::Error::other("migration worker panicked")));
        CURRENT.set(previous);
        ACTIVE.add(-1);
        ELAPSED.record(started.elapsed());
        if result.is_ok() {
            self.remember(job.key.clone());
            self.progress.notify_one();
        }
        self.queue.lock().expect("migration queue").pending.remove(&job.key);
        *job.cell.result.lock().expect("migration result") =
            Some(result.as_ref().map(|_| ()).map_err(|error| (error.kind(), error.to_string())));
        job.cell.changed.notify_all();
        result
    }
}
