//! One WAL database per service, with bounded admission and batched FULL commits.
//! A successful write has committed; a reply timeout has an unknown outcome and
//! must be reconciled using the caller's durable operation key.

mod blocking;
pub mod documents;
mod error;
mod integrity;
mod metrics;
pub mod migration;
mod queue;
mod readings;
pub mod receipts;
mod recovery;
pub mod versions;
mod writer;

pub use error::{Error, ErrorKind};
pub use metrics::Statistics;
pub use queue::Priority;
pub use rusqlite;
pub type Result<T> = std::result::Result<T, Error>;

use std::cell::Cell;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

const ADMISSION: Duration = Duration::from_secs(5);
const ANSWER: Duration = Duration::from_secs(30);
const QUEUE_WAIT: Duration = Duration::from_secs(30);

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
    reading: Arc<Semaphore>,
    background_reading: Arc<Semaphore>,
    reads: Arc<Semaphore>,
    metrics: Arc<metrics::Metrics>,
    damage: Arc<integrity::Damage>,
    path: PathBuf,
    fault: Option<Error>,
    salvaged: Option<String>,
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
        fastctx_platform::roles::blocking("opening a database");
        match Self::open_once(path, schema, &initialize) {
            // Damage the cheap checks did not see is set aside and salvaged
            // as theirs is, and the salvage opened instead.
            Err(error) if error.is_database_damage() => {
                recovery::mark_damaged(path, &error.to_string())?;
                Self::open_once(path, schema, &initialize)
            }
            opened => opened,
        }
    }

    fn open_once(path: &Path, schema: &str, initialize: &impl Fn(&rusqlite::Connection) -> Result<()>) -> Result<Self> {
        let readings = readings::Readings::of(path);
        let salvaged = recovery::prepare(path)?;
        if salvaged.is_some() {
            fastctx_platform::health::damaged(readings.name);
        }
        let parent = path.parent().ok_or_else(|| Error::new("the database has no parent directory"))?;
        // Existing directories keep their owner's ACL: a system service must
        // not take away that owner's access to an existing database.
        if !parent.try_exists()? {
            fastctx_platform::ensure_private_dir(parent)?;
        }
        let mut connection = rusqlite::Connection::open(path)?;
        connection.busy_timeout(ADMISSION)?;
        connection.set_prepared_statement_cache_capacity(64);
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "wal_autocheckpoint", 1000)?;
        connection.pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)?;
        let transaction = connection.transaction()?;
        versions::engine(&transaction)?;
        transaction.execute_batch(receipts::SCHEMA)?;
        transaction.execute_batch(documents::SCHEMA)?;
        transaction.execute_batch(schema)?;
        initialize(&transaction)?;
        transaction.commit()?;
        let count = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
        let mut readers = Vec::with_capacity(count);
        for _ in 0..count {
            let reader = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            reader.busy_timeout(ADMISSION)?;
            reader.set_prepared_statement_cache_capacity(64);
            readers.push(reader);
        }
        let writes = Arc::new(queue::Queue::new());
        let damage = Arc::new(integrity::Damage::new(path.to_path_buf(), &readings));
        let metrics = Arc::new(metrics::Metrics::new(readings));
        let (queue, measurements, noticed, file) =
            (writes.clone(), metrics.clone(), damage.clone(), path.to_path_buf());
        std::thread::Builder::new()
            .name("sqlite-writer".into())
            .spawn(move || writer::run(connection, queue, measurements, noticed, file))?;
        integrity::watch(Arc::downgrade(&damage), path.to_path_buf())?;
        Ok(Self(Arc::new(Inner {
            writes,
            readers: Mutex::new(readers),
            reading: Arc::new(Semaphore::new(count)),
            background_reading: Arc::new(Semaphore::new(count - 1)),
            reads: Arc::new(Semaphore::new(1024)),
            metrics,
            damage,
            path: path.to_path_buf(),
            fault: None,
            salvaged,
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
        match Self::open_initialized(path, schema, initialize) {
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
                    reading: Arc::new(Semaphore::new(0)),
                    background_reading: Arc::new(Semaphore::new(0)),
                    reads: Arc::new(Semaphore::new(0)),
                    metrics: Arc::new(metrics),
                    damage,
                    path: path.to_path_buf(),
                    fault: Some(fault),
                    salvaged: None,
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
        let work = tokio::task::spawn_blocking(move || {
            let _queued = queued;
            database.with_reader(permit, read)
        });
        tokio::time::timeout(ANSWER, work)
            .await
            .map_err(|_| busy("read completion"))?
            .map_err(|error| Error::new(format!("the database reader failed: {error}")))?
    }

    /// Synchronous reads belong exclusively on blocking threads. The closure may
    /// borrow its caller, and every caller shares the same connection admission.
    #[track_caller]
    pub fn read_now<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        self.reading_here();
        self.read_held(read)
    }

    /// A blocking background scan leaves one connection available to ordinary
    /// reads. Acquire its budget before leasing a connection, never afterwards.
    #[track_caller]
    pub fn read_background_now<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        self.reading_here();
        self.available()?;
        let _background = blocking::wait(self.0.background_reading.clone().acquire_owned(), QUEUE_WAIT)
            .ok_or_else(|| busy("background read admission"))?
            .map_err(|_| unavailable())?;
        self.read_held(read)
    }

    /// Notes a synchronous read asked where it must not be: on a thread that
    /// must not block, or by one that holds a read connection of this pool
    /// already, which with every connection so held waits on the others
    /// until its wait runs out. The place noted is the caller's.
    #[track_caller]
    fn reading_here(&self) {
        fastctx_platform::roles::blocking("a database read");
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
        let connection = self.0.readers.lock().expect("reader pool").pop().expect("a permit reserves a connection");
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
        self.available()?;
        let permit = tokio::time::timeout(QUEUE_WAIT, self.0.writes.admit(priority, bytes))
            .await
            .map_err(|_| busy("write admission"))??;
        self.admitted(priority);
        let (send, receive) = oneshot::channel();
        self.submit(
            priority,
            writer::job(permit, write, committed, move |result| {
                let _ = send.send(result);
            }),
        )?;
        tokio::time::timeout(ANSWER, receive).await.map_err(|_| unknown())?.map_err(|_| unknown())?
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
        fastctx_platform::roles::blocking("a database write");
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
            writer::job(permit, write, committed, move |result| {
                let _ = send.send(result);
            }),
        )?;
        blocking::wait(receive, ANSWER).ok_or_else(unknown)?.map_err(|_| unknown())?
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
