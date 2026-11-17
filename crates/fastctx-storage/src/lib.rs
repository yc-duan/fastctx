//! One WAL database per service, with bounded admission and batched FULL commits.
//! A successful write has committed; a reply timeout has an unknown outcome and
//! must be reconciled using the caller's durable operation key.

pub mod authorization;
mod blocking;
mod checkpointer;
pub mod documents;
mod error;
pub mod format_upgrade;
pub mod guard;
mod integrity;
mod keeper;
mod metrics;
pub mod migration;
mod queue;
mod readings;
pub mod receipts;
mod recovery;
mod settled;
pub mod versions;
mod writer;

pub use error::{Error, ErrorKind};
pub use guard::Guard;
pub use metrics::Statistics;
pub use queue::Priority;
pub use rusqlite;
pub type Result<T> = std::result::Result<T, Error>;

use std::cell::Cell;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

const ADMISSION: Duration = Duration::from_secs(5);
const ANSWER: Duration = Duration::from_secs(30);
const QUEUE_WAIT: Duration = Duration::from_secs(30);
/// Read connections of a database. Readers wait on the disk and on each
/// other's short queries more than on a core, and a pool as small as the
/// cores held whole machines' work behind one long read; this many cost a
/// little memory each, and keep a migration, a startup's recoveries and
/// the reads asked meanwhile from waiting for each other.
const READERS: usize = 16;
/// Those of them background reads leave to the reads someone waits for.
const INTERACTIVE_READERS: usize = 2;

/// How a database's file is laid out and held in memory, for one whose rows
/// are larger or whose writes come in larger batches than a service's
/// records. What is not given stays as SQLite has it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Shape {
    /// The size of a page. It holds for a database made by this open; one
    /// that exists keeps the size it was made with.
    pub page_bytes: Option<u32>,
    /// The pages the writer keeps in memory, in bytes.
    pub cache_bytes: Option<u32>,
    /// The pages each reader keeps in memory, in bytes: with the writer's,
    /// what the database holds at most.
    pub reader_cache_bytes: Option<u32>,
    /// The pages of log after which a commit folds the log into the database.
    pub checkpoint_pages: Option<u32>,
    /// The log folded inside the commit that takes it past `checkpoint_pages`,
    /// as SQLite folds it, not beside the writer on a thread of its own.
    /// Beside the writer a commit never waits for a fold, but the writer's
    /// own writes meet its disk work: on a slow disk one in ten took past
    /// two milliseconds (a Windows runner, 2026-11-15; on this computer's
    /// disk 9 with it inside, 34-54 beside). A drill that holds the writer's
    /// own work to that budget folds inside; every program folds beside.
    pub fold_inside: bool,
}

/// What makes a database's guard from the opened database, if it has one.
type Maker<'a> = Option<&'a dyn Fn(&rusqlite::Connection) -> Result<Box<dyn Guard>>>;

#[derive(Clone)]
pub struct Database(Arc<Inner>);

#[derive(Clone)]
pub struct WeakDatabase(std::sync::Weak<Inner>);
impl WeakDatabase {
    pub fn upgrade(&self) -> Option<Database> {
        self.0.upgrade().map(Database)
    }
}
struct Inner {
    writes: Arc<queue::Queue>,
    readers: Mutex<Vec<rusqlite::Connection>>,
    /// How a read connection is opened: one at the opening, the rest as
    /// reads come together to need them, up to `READERS` — an idle database
    /// keeps no more connections than it ever read with at once (§18 idle
    /// memory, codex-865aeb9b: about 10 MiB of a node's working set).
    opening: Option<ReaderOpening>,
    reading: Arc<Semaphore>,
    background_reading: Arc<Semaphore>,
    reads: Arc<Semaphore>,
    metrics: Arc<metrics::Metrics>,
    damage: Arc<integrity::Damage>,
    path: PathBuf,
    fault: Option<Error>,
    salvaged: Option<String>,
    /// Whether a relaxed commit may not have reached the disk yet; set by
    /// the writer, cleared once a settle commits (`settle_now`).
    unsettled: Arc<AtomicBool>,
    /// Shared with the writer thread, which lets it go after its connection.
    _keeper: Option<Arc<keeper::Keeper>>,
    upgrade: std::sync::OnceLock<format_upgrade::MigrationGate>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.writes.close();
    }
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").finish_non_exhaustive()
    }
}

/// What opens a read connection of a database.
struct ReaderOpening {
    path: PathBuf,
    cache_bytes: Option<u32>,
    tables: Arc<std::collections::HashSet<String>>,
    policy: Option<Arc<dyn authorization::WritePolicy>>,
}

impl ReaderOpening {
    fn open(&self) -> Result<rusqlite::Connection> {
        let mut reader = rusqlite::Connection::open_with_flags(
            &self.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        reader.busy_timeout(ADMISSION)?;
        reader.set_prepared_statement_cache_capacity(64);
        if let Some(cache) = self.cache_bytes {
            reader.pragma_update(None, "cache_size", -i64::from((cache / 1024).max(1)))?;
        }
        authorization::install(&mut reader, self.tables.clone(), false, self.policy.clone())?;
        Ok(reader)
    }
}

struct Reader<'a> {
    inner: &'a Inner,
    connection: Option<rusqlite::Connection>,
    _permit: OwnedSemaphorePermit,
}

thread_local! {
    /// The read connections, of any database, this thread holds now.
    static HOLDING: Cell<u32> = const { Cell::new(0) };
}

/// A read connection held on this thread, until dropped.
struct Holding;

impl Holding {
    fn begin() -> Self {
        HOLDING.with(|held| held.set(held.get() + 1));
        Self
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        HOLDING.with(|held| held.set(held.get() - 1));
    }
}
impl Drop for Reader<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.inner.readers.lock().expect("reader pool").push(connection);
        }
    }
}

impl Database {
    pub fn downgrade(&self) -> WeakDatabase {
        WeakDatabase(Arc::downgrade(&self.0))
    }

    /// Opens a private database. Call on a blocking thread, without business locks.
    #[track_caller]
    pub fn open(path: &Path, schema: &str) -> Result<Self> {
        Self::open_initialized(path, schema, |_| Ok(()))
    }

    /// Installs the schema and initializer atomically. Legacy callers retain
    /// this entry point; services with multiple versions use `versions::apply`.
    #[track_caller]
    pub fn open_initialized(
        path: &Path,
        schema: &str,
        initialize: impl Fn(&rusqlite::Connection) -> Result<()>,
    ) -> Result<Self> {
        Self::open_kept(path, schema, &initialize, None, Shape::default())
    }

    /// Opens as `open` does, with the file laid out and cached as `shape` says.
    #[track_caller]
    pub fn open_shaped(path: &Path, schema: &str, shape: Shape) -> Result<Self> {
        Self::open_kept(path, schema, &|_| Ok(()), None, shape)
    }

    /// Opens as `open_initialized` does, with the file laid out and cached as
    /// `shape` says.
    #[track_caller]
    pub fn open_initialized_shaped(
        path: &Path,
        schema: &str,
        shape: Shape,
        initialize: impl Fn(&rusqlite::Connection) -> Result<()>,
    ) -> Result<Self> {
        Self::open_kept(path, schema, &initialize, None, shape)
    }

    /// Opens as `open_initialized`, the writer keeping the rules of `guard`,
    /// made from the opened database, on every row it changes.
    #[track_caller]
    pub fn open_guarded(
        path: &Path,
        schema: &str,
        guard: impl Fn(&rusqlite::Connection) -> Result<Box<dyn Guard>>,
        initialize: impl Fn(&rusqlite::Connection) -> Result<()>,
    ) -> Result<Self> {
        Self::open_kept(path, schema, &initialize, Some(&guard), Shape::default())
    }

    /// Opens as `open_guarded` does, with the file laid out, cached and its
    /// log folded as `shape` says.
    #[track_caller]
    pub fn open_guarded_shaped(
        path: &Path,
        schema: &str,
        shape: Shape,
        guard: impl Fn(&rusqlite::Connection) -> Result<Box<dyn Guard>>,
        initialize: impl Fn(&rusqlite::Connection) -> Result<()>,
    ) -> Result<Self> {
        Self::open_kept(path, schema, &initialize, Some(&guard), shape)
    }

    #[track_caller]
    fn open_kept(
        path: &Path,
        schema: &str,
        initialize: &dyn Fn(&rusqlite::Connection) -> Result<()>,
        guard: Maker<'_>,
        shape: Shape,
    ) -> Result<Self> {
        fastctx_platform::roles::in_place("opening a database", || {
            let parent = path.parent().ok_or_else(|| Error::new("the database has no parent directory"))?;
            // Existing directories keep their owner's ACL: a system service must
            // not take away that owner's access to an existing database.
            if !parent.try_exists()? {
                fastctx_platform::ensure_private_dir(parent)?;
            }
            let kept = keeper::keep(path)?;
            match Self::open_once(path, schema, initialize, guard, &kept, shape) {
                // Damage the cheap checks did not see is set aside and salvaged
                // as theirs is, and the salvage opened instead; only when no
                // other connection to it is open.
                Err(error) if error.is_database_damage() && kept.sole => {
                    recovery::mark_damaged(path, &error.to_string())?;
                    Self::open_once(path, schema, initialize, guard, &kept, shape)
                }
                opened => opened,
            }
        })
    }

    fn open_once(
        path: &Path,
        schema: &str,
        initialize: &dyn Fn(&rusqlite::Connection) -> Result<()>,
        guard: Maker<'_>,
        kept: &keeper::Kept,
        shape: Shape,
    ) -> Result<Self> {
        let readings = readings::Readings::of(path);
        // Another handle of this process has it open, or is closing it: its
        // file is not looked at under it.
        let salvaged = if kept.sole { recovery::prepare(path)? } else { None };
        if salvaged.is_some() {
            fastctx_platform::health::damaged(readings.name);
        }
        let mut connection = rusqlite::Connection::open(path)?;
        connection.busy_timeout(ADMISSION)?;
        connection.set_prepared_statement_cache_capacity(64);
        if let Some(page) = shape.page_bytes {
            // Before the log's mode: a database in it keeps its page size.
            connection.pragma_update(None, "page_size", page)?;
        }
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "wal_autocheckpoint", shape.checkpoint_pages.unwrap_or(1000))?;
        if let Some(cache) = shape.cache_bytes {
            connection.pragma_update(None, "cache_size", -i64::from(cache / 1024))?;
        }
        connection.pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)?;
        let transaction = connection.transaction()?;
        versions::engine(&transaction)?;
        transaction.execute_batch(receipts::SCHEMA)?;
        transaction.execute_batch(documents::SCHEMA)?;
        transaction.execute_batch(schema)?;
        initialize(&transaction)?;
        transaction.commit()?;
        let guard: Option<guard::Shared> = match guard {
            Some(make) => Some(Arc::new(Mutex::new(make(&connection)?))),
            None => None,
        };
        let tables = authorization::tables(&connection)?;
        let policy = guard.as_ref().and_then(|guard| guard::lock(guard).write_policy());
        authorization::install(&mut connection, tables.clone(), true, policy.clone())?;
        let count = READERS;
        let opening = ReaderOpening { path: path.to_path_buf(), cache_bytes: shape.reader_cache_bytes, tables: tables.clone(), policy: policy.clone() };
        // One opened now, so that one that cannot be opened fails the opening.
        let readers = vec![opening.open()?];
        let writes = Arc::new(queue::Queue::new());
        let damage = Arc::new(integrity::Damage::new(path.to_path_buf(), &readings));
        let metrics = Arc::new(metrics::Metrics::new(readings));
        // What a process before this one committed without waiting for the
        // disk may not be on it yet, its log still in the system's memory
        // after the process ended: the first settle synchronizes it.
        let unsettled = Arc::new(AtomicBool::new(true));
        // After `wal_autocheckpoint`, whose fold inside the commit the hook
        // takes the place of. A hook not put on fails the opening: the
        // connection may keep a hook whose state was freed.
        let ending = if shape.fold_inside {
            checkpointer::inside(path, kept.keeper.clone())
        } else {
            checkpointer::install(&mut connection, path, shape.checkpoint_pages.unwrap_or(1000), kept.keeper.clone())?
        };
        let (queue, measurements, noticed, file, relaxed) =
            (writes.clone(), metrics.clone(), damage.clone(), path.to_path_buf(), unsettled.clone());
        std::thread::Builder::new().name("sqlite-writer".into()).spawn(move || {
            // Dropped as the writer ends, unwinding too, after its connection
            // and with it the last checkpoint: the thread that folded the log
            // beside it is gone, then the file's keeper is let go.
            let _ending = ending;
            writer::run(connection, queue, measurements, noticed, file, relaxed, guard);
        })?;
        integrity::watch(Arc::downgrade(&damage), path.to_path_buf())?;
        Ok(Self(Arc::new(Inner {
            writes,
            readers: Mutex::new(readers),
            opening: Some(opening),
            reading: Arc::new(Semaphore::new(count)),
            background_reading: Arc::new(Semaphore::new(count - INTERACTIVE_READERS)),
            reads: Arc::new(Semaphore::new(1024)),
            metrics,
            damage,
            path: path.to_path_buf(),
            fault: None,
            salvaged,
            unsettled,
            _keeper: Some(kept.keeper.clone()),
            upgrade: std::sync::OnceLock::new(),
        })))
    }

    /// Opens without making a service's diagnostic endpoint depend on storage:
    /// a database that does not open is a handle that rejects every operation
    /// with its cause. A damaged one has been set aside and salvaged by then
    /// (`open_initialized`); its files are kept, never rebuilt empty unsaid.
    /// The caller must exclusively own the service database during startup.
    #[track_caller]
    pub fn open_or_unavailable(
        path: &Path,
        schema: &str,
        initialize: impl Fn(&rusqlite::Connection) -> Result<()>,
    ) -> Self {
        Self::or_unavailable(path, Self::open_kept(path, schema, &initialize, None, Shape::default()))
    }

    /// Opens as `open_or_unavailable`, the writer keeping `guard`'s rules.
    #[track_caller]
    pub fn open_guarded_or_unavailable(
        path: &Path,
        schema: &str,
        guard: impl Fn(&rusqlite::Connection) -> Result<Box<dyn Guard>>,
        initialize: impl Fn(&rusqlite::Connection) -> Result<()>,
    ) -> Self {
        Self::or_unavailable(path, Self::open_kept(path, schema, &initialize, Some(&guard), Shape::default()))
    }

    /// Keeps startup refusal available to diagnostics without touching the file.
    pub fn refused(path: &Path, error: Error) -> Self {
        Self::or_unavailable(path, Err(error))
    }

    fn or_unavailable(path: &Path, opened: Result<Self>) -> Self {
        match opened {
            Ok(database) => database,
            Err(fault) => {
                let writes = Arc::new(queue::Queue::new());
                writes.close();
                let readings = readings::Readings::of(path);
                let damage = Arc::new(integrity::Damage::new(path.to_path_buf(), &readings));
                let metrics = metrics::Metrics::new(readings);
                metrics.failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Self(Arc::new(Inner {
                    writes,
                    readers: Mutex::new(Vec::new()),
                    opening: None,
                    reading: Arc::new(Semaphore::new(0)),
                    background_reading: Arc::new(Semaphore::new(0)),
                    reads: Arc::new(Semaphore::new(0)),
                    metrics: Arc::new(metrics),
                    damage,
                    path: path.to_path_buf(),
                    fault: Some(fault),
                    salvaged: None,
                    unsettled: Arc::new(AtomicBool::new(false)),
                    _keeper: None,
                    upgrade: std::sync::OnceLock::new(),
                }))
            }
        }
    }

    /// What was salvaged when this open found the database damaged and set it
    /// aside: each table's rows that came over and why the rest did not.
    pub fn salvaged(&self) -> Option<&str> {
        self.0.salvaged.as_deref()
    }

    /// A startup failure remains explicit until storage is repaired and reopened.
    pub fn fault(&self) -> Option<&Error> {
        self.0.fault.as_ref()
    }
    fn available(&self) -> Result<()> {
        match &self.0.fault {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }

    pub fn statistics(&self) -> Statistics {
        let mut statistics = self.0.metrics.snapshot();
        statistics.unavailable = self.fault().map(ToString::to_string);
        statistics
    }

    /// Executes a read on a bounded pool. Waiting for a connection never polls.
    pub async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.available()?;
        let asked = Instant::now();
        let admitted = async {
            let queued = tokio::time::timeout(QUEUE_WAIT, self.0.reads.clone().acquire_owned())
                .await
                .map_err(|_| busy("read admission"))?
                .map_err(|_| unavailable())?;
            let permit = tokio::time::timeout(QUEUE_WAIT, self.0.reading.clone().acquire_owned())
                .await
                .map_err(|_| busy("read connection"))?
                .map_err(|_| unavailable())?;
            Ok::<_, Error>((queued, permit))
        }
        .await;
        self.0.metrics.read_wait(asked.elapsed());
        let (queued, permit) = admitted?;
        let database = self.clone();
        let work = fastctx_platform::task::spawn_blocking(move || {
            let _queued = queued;
            database.with_reader(permit, read)
        });
        tokio::time::timeout(ANSWER, work)
            .await
            .map_err(|_| busy("read completion"))?
            .map_err(|error| Error::new(format!("the database reader failed: {error}")))?
    }

    /// A synchronous read blocks its thread: on a task thread it is done in
    /// place (`fastctx_platform::roles::in_place`). The closure may borrow its
    /// caller, and every caller shares the same connection admission.
    #[track_caller]
    pub fn read_now<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        self.reading_here();
        fastctx_platform::roles::in_place("a database read", || self.read_held(read))
    }

    /// A blocking background scan leaves one connection available to ordinary
    /// reads. Acquire its budget before leasing a connection, never afterwards.
    #[track_caller]
    pub fn read_background_now<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        self.reading_here();
        fastctx_platform::roles::in_place("a database read", || {
            self.available()?;
            let _background = blocking::wait(self.0.background_reading.clone().acquire_owned(), QUEUE_WAIT)
                .ok_or_else(|| busy("background read admission"))?
                .map_err(|_| unavailable())?;
            self.read_held(read)
        })
    }

    /// Notes a synchronous read asked by a thread that holds a read
    /// connection of this pool already, which with every connection so held
    /// waits on the others until its wait runs out. The place noted is the
    /// caller's.
    #[track_caller]
    fn reading_here(&self) {
        if HOLDING.with(Cell::get) > 0 {
            let name = self.0.path.file_name().unwrap_or(self.0.path.as_os_str()).to_string_lossy();
            fastctx_platform::conduct::nested_read(&name, Location::caller());
        }
    }

    fn read_held<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        self.available()?;
        let asked = Instant::now();
        let permit = blocking::wait(self.0.reading.clone().acquire_owned(), QUEUE_WAIT);
        self.0.metrics.read_wait(asked.elapsed());
        let permit = permit.ok_or_else(|| busy("read connection"))?.map_err(|_| unavailable())?;
        self.with_reader(permit, read)
    }

    fn with_reader<T>(
        &self,
        permit: OwnedSemaphorePermit,
        read: impl FnOnce(&rusqlite::Connection) -> Result<T>,
    ) -> Result<T> {
        fastctx_platform::cost::database_read();
        // A permit holds a place for a connection: one kept, or one opened
        // now, never more than `READERS` in all.
        let kept = self.0.readers.lock().expect("reader pool").pop();
        let connection = match kept {
            Some(connection) => connection,
            None => self.0.opening.as_ref().ok_or_else(unavailable)?.open()?,
        };
        let reader = Reader { inner: &self.0, connection: Some(connection), _permit: permit };
        let _holding = Holding::begin();
        let start = Instant::now();
        let result = read(reader.connection.as_ref().expect("leased connection"));
        self.0.metrics.read(start.elapsed());
        if let Err(error) = &result {
            self.0.damage.notice(error);
        }
        result
    }

    /// Runs an atomic SQL-only operation. Input bytes include all retained data.
    pub async fn write<T: Send + 'static>(
        &self,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.write_at(Priority::Normal, bytes, write).await
    }

    pub async fn write_at<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.write_then_at(priority, bytes, write, |_| {}).await
    }

    /// Publishes after FULL commit, even if the awaiting caller went away.
    /// `committed` may notify or replace memory, but must not perform I/O or take
    /// the caller's business locks. It runs exactly once after a successful commit.
    pub async fn write_then<T: Send + 'static>(
        &self,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        committed: impl FnOnce(&T) + Send + 'static,
    ) -> Result<T> {
        self.write_then_at(Priority::Normal, bytes, write, committed).await
    }

    pub async fn write_then_at<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        committed: impl FnOnce(&T) + Send + 'static,
    ) -> Result<T> {
        self.write_committed_at(priority, bytes, true, write, committed).await
    }

    /// Publishes after commit without requiring a disk synchronization. The
    /// notification survives cancellation or timeout of the awaiting caller.
    pub async fn write_relaxed_then_at<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        committed: impl FnOnce(&T) + Send + 'static,
    ) -> Result<T> {
        self.write_committed_at(priority, bytes, false, write, committed).await
    }

    async fn write_committed_at<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        durable: bool,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        committed: impl FnOnce(&T) + Send + 'static,
    ) -> Result<T> {
        self.available()?;
        let permit = tokio::time::timeout(QUEUE_WAIT, self.0.writes.admit(priority, bytes))
            .await
            .map_err(|_| busy("write admission"))??;
        self.admitted(priority);
        let (send, receive) = oneshot::channel();
        self.submit(
            priority,
            writer::job(permit, durable, write, committed, move |result| {
                let _ = send.send(result);
            }),
        )?;
        tokio::time::timeout(ANSWER, receive).await.map_err(|_| unknown())?.map_err(|_| unknown())?
    }

    /// Commits as `write` does without waiting for the disk, as
    /// `write_relaxed_now_at` says: for what a later durable commit of the
    /// same work makes whole, and what is done again, unharmed, when lost.
    pub async fn write_relaxed<T: Send + 'static>(
        &self,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.write_relaxed_then_at(Priority::Normal, bytes, write, |_| {}).await
    }

    /// Blocking-thread compatibility; it parks on the same admission and commit
    /// notifications as asynchronous callers, never sleeps in a polling loop.
    #[track_caller]
    pub fn write_now<T: Send + 'static>(
        &self,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.write_now_at(Priority::Normal, bytes, write)
    }

    #[track_caller]
    pub fn write_now_at<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.write_now_then_at(priority, bytes, write, |_| {})
    }

    /// A blocking caller parks until commit. Its memory-only notification runs
    /// in writer order after durability, even if the caller no longer waits.
    #[track_caller]
    pub fn write_now_then_at<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        committed: impl FnOnce(&T) + Send + 'static,
    ) -> Result<T> {
        self.commit_now(priority, bytes, true, write, committed)
    }

    /// Commits as `write_now_at` does without waiting for the disk: the
    /// change is committed and seen at once, and kept when the process
    /// stops, but a machine that loses its power may lose it until a later
    /// durable commit or `settle_now`. For work done again, unharmed, when
    /// its result is lost, such as bringing records over from older files.
    #[track_caller]
    pub fn write_relaxed_now_at<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.commit_now(priority, bytes, false, write, |_| {})
    }

    /// Commits each of `writes` in order and durably, as `write_now_at`
    /// commits one, without waiting for one before giving the writer the
    /// next: those it finds waiting it takes into one transaction, each in a
    /// savepoint of its own, so that one disk synchronization serves them
    /// all. Each sees what those before it wrote, and stands on them: once
    /// one fails — its write, by an error or a panic, or the commit of the
    /// transaction it was in — none after it writes anything. The results,
    /// in order; once one cannot be given to the writer, none after it is.
    /// The writes are admitted within one wait, and answered within one, as
    /// one write is.
    #[track_caller]
    #[allow(clippy::type_complexity)]
    pub fn write_many_now_at<T: Send + 'static>(
        &self,
        priority: Priority,
        writes: Vec<(usize, Box<dyn FnOnce(&rusqlite::Connection) -> Result<T> + Send>)>,
    ) -> Vec<Result<T>> {
        fastctx_platform::roles::in_place("database writes", || {
            let total = writes.len();
            let refused = |error: Error| -> Vec<Result<T>> { (0..total).map(|_| Err(error.clone())).collect() };
            if let Err(error) = self.available() {
                return refused(error);
            }
            if writer::on_writer() {
                debug_assert!(false, "a database writer waited for a write");
                return refused(Error::with_kind(ErrorKind::Invalid, "a database's writer cannot wait for a write"));
            }
            let mut answers = Vec::with_capacity(total);
            // Set while a write of the run has not stood — set as it begins,
            // cleared once its savepoint is kept, so an error, a panic or a
            // guard's refusal leaves it set — and by the answer of one whose
            // commit failed: the writer answers a transaction's writes before
            // it begins the next.
            let failed = Arc::new(AtomicBool::new(false));
            let admitted_by = Instant::now() + QUEUE_WAIT;
            for (index, (bytes, write)) in writes.into_iter().enumerate() {
                let left = admitted_by.saturating_duration_since(Instant::now());
                let permit = match blocking::wait(self.0.writes.admit(priority, bytes), left) {
                    Some(Ok(permit)) => permit,
                    Some(Err(error)) => {
                        answers.push(Err(error));
                        break;
                    }
                    None => {
                        answers.push(Err(busy("write admission")));
                        break;
                    }
                };
                self.admitted(priority);
                let (send, receive) = oneshot::channel();
                let (stands, kept, answered) = (failed.clone(), failed.clone(), failed.clone());
                let write = move |connection: &rusqlite::Connection| {
                    if stands.load(Ordering::Acquire) {
                        return Err(Error::with_kind(ErrorKind::Invalid, "not written: a write before it in the run failed"));
                    }
                    stands.store(true, Ordering::Release);
                    write(connection)
                };
                let stood = move || kept.store(false, Ordering::Release);
                let job = writer::job_in_run(permit, true, index + 1 < total, stood, write, |_: &T| {}, move |result: Result<T>| {
                    if result.is_err() {
                        answered.store(true, Ordering::Release);
                    }
                    let _ = send.send(result);
                });
                if let Err(error) = self.submit(priority, job) {
                    answers.push(Err(error));
                    break;
                }
                answers.push(Ok(receive));
            }
            // One deadline for the run, as for one write: a stuck writer is
            // told within it, not once for each.
            let deadline = Instant::now() + ANSWER;
            let mut results: Vec<Result<T>> = answers
                .into_iter()
                .map(|answer| match answer {
                    Ok(receive) => {
                        let left = deadline.saturating_duration_since(Instant::now());
                        blocking::wait(receive, left).ok_or_else(unknown)?.map_err(|_| unknown())?
                    }
                    Err(error) => Err(error),
                })
                .collect();
            while results.len() < total {
                results.push(Err(busy("an earlier write of the batch")));
            }
            results
        })
    }

    /// Makes every relaxed write committed so far as durable as any other,
    /// before something is done that only they would undo; at once when
    /// none is waiting for the disk.
    #[track_caller]
    pub fn settle_now(&self, priority: Priority) -> Result<()> {
        if !self.0.unsettled.load(Ordering::Acquire) {
            return Ok(());
        }
        let unsettled = self.0.unsettled.clone();
        // A durable commit syncs the log, and every relaxed commit before
        // it with it; the engine's version, written again as it is, gives it
        // a page to write.
        self.commit_now(
            priority,
            64,
            true,
            |db| {
                let version: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
                db.pragma_update(None, "user_version", version)?;
                Ok(())
            },
            move |()| unsettled.store(false, Ordering::Release),
        )
    }

    #[track_caller]
    fn commit_now<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        durable: bool,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        committed: impl FnOnce(&T) + Send + 'static,
    ) -> Result<T> {
        fastctx_platform::roles::in_place("a database write", || {
            self.available()?;
            if writer::on_writer() {
                // A writer waiting for a write waits for itself, or for another
                // database's writer while every write of its own waits for it.
                debug_assert!(false, "a database writer waited for a write");
                return Err(Error::with_kind(ErrorKind::Invalid, "a database's writer cannot wait for a write"));
            }
            let permit = blocking::wait(self.0.writes.admit(priority, bytes), QUEUE_WAIT)
                .ok_or_else(|| busy("write admission"))??;
            self.admitted(priority);
            let (send, receive) = oneshot::channel();
            self.submit(
                priority,
                writer::job(permit, durable, write, committed, move |result| {
                    let _ = send.send(result);
                }),
            )?;
            blocking::wait(receive, ANSWER).ok_or_else(unknown)?.map_err(|_| unknown())?
        })
    }

    fn submit(&self, priority: Priority, job: writer::Job) -> Result<()> {
        self.0.metrics.queue(1);
        let result = self.0.writes.push(priority, job);
        if result.is_err() {
            self.0.metrics.queue(-1);
        }
        result
    }

    /// Notes how much of its priority's admission a write found in use.
    fn admitted(&self, priority: Priority) {
        self.0.metrics.readings.writes.note(self.0.writes.used(priority), queue::SLOTS);
    }
}

fn busy(what: &str) -> Error {
    Error::with_kind(ErrorKind::Busy, format!("the database timed out waiting for {what}"))
}
fn unavailable() -> Error {
    Error::with_kind(ErrorKind::Unavailable, "the database reader stopped")
}
fn unknown() -> Error {
    Error::with_kind(
        ErrorKind::OutcomeUnknown,
        "the database did not acknowledge its write; whether it committed is unknown",
    )
}
