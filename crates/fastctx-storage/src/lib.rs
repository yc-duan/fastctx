//! One WAL database per service, with bounded admission and batched commits.
//! Callers supply SQL-only operations. A successful write is acknowledged only
//! after the whole batch commits with synchronous=FULL; a timed-out write has
//! an unknown outcome and must be reconciled using its durable operation key.

pub mod documents;
pub mod migration;
pub mod receipts;
mod writer;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc, oneshot};

pub use rusqlite;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct Error(Arc<str>);
impl Error {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(Arc::from(reason.into()))
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::new(error.to_string())
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

const QUEUED: usize = 1024;
const BYTES: u32 = 64 << 20;
const ADMISSION: Duration = Duration::from_secs(5);
const ANSWER: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Database(Arc<Inner>);
struct Inner {
    writes: mpsc::Sender<writer::Job>,
    bytes: Arc<Semaphore>,
    readers: Vec<Mutex<rusqlite::Connection>>,
    reading: Arc<Semaphore>,
    reads: Arc<Semaphore>,
    next: AtomicUsize,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").finish_non_exhaustive()
    }
}

impl Database {
    /// Opens a private database and installs the service's idempotent schema.
    /// Opening belongs in startup or a blocking task, never under a shared lock.
    pub fn open(path: &Path, schema: &str) -> Result<Self> {
        let parent = path.parent().ok_or_else(|| Error::new("the database has no parent directory"))?;
        fastctx_platform::ensure_private_dir(parent)?;
        let mut connection = rusqlite::Connection::open(path)?;
        connection.busy_timeout(ADMISSION)?;
        connection.set_prepared_statement_cache_capacity(64);
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "wal_autocheckpoint", 1000)?;
        let transaction = connection.transaction()?;
        transaction.execute_batch(receipts::SCHEMA)?;
        transaction.execute_batch(documents::SCHEMA)?;
        transaction.execute_batch(schema)?;
        transaction.commit()?;
        let count = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
        let mut readers = Vec::with_capacity(count);
        for _ in 0..count {
            let reader = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            reader.busy_timeout(ADMISSION)?;
            reader.set_prepared_statement_cache_capacity(32);
            readers.push(Mutex::new(reader));
        }
        let (writes, receive) = mpsc::channel(QUEUED);
        std::thread::Builder::new().name("sqlite-writer".into()).spawn(move || writer::run(connection, receive))?;
        Ok(Self(Arc::new(Inner {
            writes,
            bytes: Arc::new(Semaphore::new(BYTES as usize)),
            readers,
            reading: Arc::new(Semaphore::new(count)),
            reads: Arc::new(Semaphore::new(QUEUED)),
            next: AtomicUsize::new(0),
        })))
    }

    /// Executes one read on a bounded pool. A read must not modify the database.
    pub async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let queued =
            self.0.reads.clone().try_acquire_owned().map_err(|_| Error::new("the database read queue is full"))?;
        let permit = tokio::time::timeout(ADMISSION, self.0.reading.clone().acquire_owned())
            .await
            .map_err(|_| Error::new("the database readers stayed busy for 5 s"))?
            .map_err(|_| Error::new("the database reader stopped"))?;
        let database = self.clone();
        let work = tokio::task::spawn_blocking(move || {
            let _permit = (permit, queued);
            database.read_now(read)
        });
        tokio::time::timeout(ANSWER, work)
            .await
            .map_err(|_| Error::new("the database read did not finish within 30 s"))?
            .map_err(|error| Error::new(format!("the database reader failed: {error}")))?
    }

    /// A short indexed read for synchronous record paths and migration. Async
    /// entry points use `read` so contention does not occupy runtime workers.
    pub fn read_now<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        let slot = self.0.next.fetch_add(1, Ordering::Relaxed) % self.0.readers.len();
        let deadline = std::time::Instant::now() + ADMISSION;
        loop {
            for offset in 0..self.0.readers.len() {
                match self.0.readers[(slot + offset) % self.0.readers.len()].try_lock() {
                    Ok(reader) => return read(&reader),
                    Err(std::sync::TryLockError::Poisoned(_)) => {
                        return Err(Error::new("the database reader panicked"));
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {}
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::new("the database readers stayed busy for 5 s"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Runs an atomic SQL operation. `bytes` bounds retained input; the closure
    /// performs no external effects and must not commit the enclosing transaction.
    pub async fn write<T: Send + 'static>(
        &self,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.write_then(bytes, write, |_| {}).await
    }

    /// Runs `committed` exactly once after commit, even if the caller went away.
    /// This callback only publishes in-memory state or notifications. Short leaf
    /// locks are allowed; it must not perform I/O, serialize payloads, or acquire
    /// a caller's business-state lock. Expensive follow-up work runs elsewhere.
    pub async fn write_then<T: Send + 'static>(
        &self,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        committed: impl FnOnce(&T) + Send + 'static,
    ) -> Result<T> {
        let amount = u32::try_from(bytes.saturating_add(512))
            .ok()
            .filter(|n| *n <= BYTES)
            .ok_or_else(|| Error::new("the database operation exceeds its input budget"))?;
        let permit = self
            .0
            .bytes
            .clone()
            .try_acquire_many_owned(amount)
            .map_err(|_| Error::new("the database input budget is full"))?;
        let (send, receive) = oneshot::channel();
        let job = writer::job(permit, write, committed, move |result| {
            let _ = send.send(result);
        });
        self.0.writes.try_send(job).map_err(|_| Error::new("the database write queue is unavailable"))?;
        tokio::time::timeout(ANSWER, receive)
            .await
            .map_err(|_| Error::new("the database write did not answer within 30 s; whether it committed is unknown"))?
            .map_err(|_| Error::new("the database writer stopped before acknowledging its commit"))?
    }

    /// Synchronous compatibility for existing filesystem-record entry points.
    /// It joins the same commit queue; it never starts a second writer connection.
    pub fn write_now<T: Send + 'static>(
        &self,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let amount = u32::try_from(bytes.saturating_add(512))
            .ok()
            .filter(|n| *n <= BYTES)
            .ok_or_else(|| Error::new("the database operation exceeds its input budget"))?;
        let permit = self
            .0
            .bytes
            .clone()
            .try_acquire_many_owned(amount)
            .map_err(|_| Error::new("the database input budget is full"))?;
        let (send, receive) = std::sync::mpsc::channel();
        let job = writer::job(
            permit,
            write,
            |_| {},
            move |result| {
                let _ = send.send(result);
            },
        );
        self.0.writes.try_send(job).map_err(|_| Error::new("the database write queue is unavailable"))?;
        receive
            .recv_timeout(ANSWER)
            .map_err(|_| Error::new("the database write did not answer within 30 s; whether it committed is unknown"))?
    }
}
