//! One WAL database per service, with bounded admission and batched FULL commits.
//! A successful write has committed; a reply timeout has an unknown outcome and
//! must be reconciled using the caller's durable operation key.

mod blocking;
pub mod documents;
mod error;
mod metrics;
pub mod migration;
mod queue;
pub mod receipts;
mod recovery;
pub mod versions;
mod writer;

pub use error::{Error, ErrorKind};
pub use metrics::Statistics;
pub use queue::Priority;
pub use rusqlite;
pub type Result<T> = std::result::Result<T, Error>;

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
    reads: Arc<Semaphore>,
    metrics: Arc<metrics::Metrics>,
    path: PathBuf,
    fault: Option<Error>,
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
    pub fn open(path: &Path, schema: &str) -> Result<Self> {
        Self::open_initialized(path, schema, |_| Ok(()))
    }

    /// Installs the schema and initializer atomically. Legacy callers retain
    /// this entry point; services with multiple versions use `versions::apply`.
    pub fn open_initialized(
        path: &Path,
        schema: &str,
        initialize: impl FnOnce(&rusqlite::Connection) -> Result<()>,
    ) -> Result<Self> {
        recovery::check(path)?;
        recovery::verify(path)?;
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
        let metrics = Arc::new(metrics::Metrics::default());
        let (queue, measurements, file) = (writes.clone(), metrics.clone(), path.to_path_buf());
        std::thread::Builder::new()
            .name("sqlite-writer".into())
            .spawn(move || writer::run(connection, queue, measurements, file))?;
        Ok(Self(Arc::new(Inner {
            writes,
            readers: Mutex::new(readers),
            reading: Arc::new(Semaphore::new(count)),
            reads: Arc::new(Semaphore::new(1024)),
            metrics,
            path: path.to_path_buf(),
            fault: None,
        })))
    }

    /// Opens without making a service's diagnostic endpoint depend on storage.
    /// A failed handle rejects every operation with its recorded cause. Only
    /// native SQLite damage is quarantined; no acknowledged state is rebuilt.
    /// The caller must exclusively own the service database during startup.
    pub fn open_or_unavailable(
        path: &Path,
        schema: &str,
        initialize: impl FnOnce(&rusqlite::Connection) -> Result<()>,
    ) -> Self {
        match Self::open_initialized(path, schema, initialize) {
            Ok(database) => database,
            Err(error) => {
                let fault = if error.is_database_damage() {
                    match recovery::quarantine(path, &error) {
                        Ok(target) => Error::damaged(format!(
                            "the database is unavailable; preserved in {}: {error}",
                            target.display()
                        )),
                        Err(failure) => Error::damaged(format!(
                            "the database is unavailable ({error}); preserving its files also failed: {failure}"
                        )),
                    }
                } else {
                    error
                };
                let writes = Arc::new(queue::Queue::new());
                writes.close();
                Self(Arc::new(Inner {
                    writes,
                    readers: Mutex::new(Vec::new()),
                    reading: Arc::new(Semaphore::new(0)),
                    reads: Arc::new(Semaphore::new(0)),
                    metrics: Arc::new(metrics::Metrics {
                        failures: std::sync::atomic::AtomicU64::new(1),
                        ..Default::default()
                    }),
                    path: path.to_path_buf(),
                    fault: Some(fault),
                }))
            }
        }
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
        let queued = tokio::time::timeout(QUEUE_WAIT, self.0.reads.clone().acquire_owned())
            .await
            .map_err(|_| busy("read admission"))?
            .map_err(|_| unavailable())?;
        let permit = tokio::time::timeout(QUEUE_WAIT, self.0.reading.clone().acquire_owned())
            .await
            .map_err(|_| busy("read connection"))?
            .map_err(|_| unavailable())?;
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
    pub fn read_now<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        self.available()?;
        let permit = blocking::wait(self.0.reading.clone().acquire_owned(), QUEUE_WAIT)
            .ok_or_else(|| busy("read connection"))?
            .map_err(|_| unavailable())?;
        self.with_reader(permit, read)
    }

    fn with_reader<T>(
        &self,
        permit: OwnedSemaphorePermit,
        read: impl FnOnce(&rusqlite::Connection) -> Result<T>,
    ) -> Result<T> {
        let connection = self.0.readers.lock().expect("reader pool").pop().expect("a permit reserves a connection");
        let reader = Reader { inner: &self.0, connection: Some(connection), _permit: permit };
        let start = Instant::now();
        let result = read(reader.connection.as_ref().expect("leased connection"));
        self.0.metrics.read_micros.record(start.elapsed());
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
    pub fn write_now<T: Send + 'static>(
        &self,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.write_now_at(Priority::Normal, bytes, write)
    }

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
    pub fn write_now_then_at<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        committed: impl FnOnce(&T) + Send + 'static,
    ) -> Result<T> {
        self.available()?;
        let permit = blocking::wait(self.0.writes.admit(priority, bytes), QUEUE_WAIT)
            .ok_or_else(|| busy("write admission"))??;
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
        use std::sync::atomic::Ordering;
        self.0.metrics.queued.fetch_add(1, Ordering::Relaxed);
        let result = self.0.writes.push(priority, job);
        if result.is_err() {
            self.0.metrics.queued.fetch_sub(1, Ordering::Relaxed);
        }
        result
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
