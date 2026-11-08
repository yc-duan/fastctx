//! A single writer batches ready operations without delaying an idle caller.

use super::{Error, ErrorKind, Result};
use crate::{
    guard,
    integrity::Damage,
    metrics::Metrics,
    queue::{Admission, Queue},
};
use rusqlite::{Connection, TransactionBehavior};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(super) type Job = Box<dyn Work>;
pub(super) trait Work: Send {
    fn apply(&mut self, connection: &Connection) -> bool;
    /// Why the operation failed, once applied.
    fn failure(&self) -> Option<&Error>;
    /// Fails the applied operation with why its changes were refused.
    fn refuse(&mut self, why: Error);
    /// Whether its commit reaches the disk before it is answered. A relaxed
    /// one is committed and seen at once, and reaches the disk with the next
    /// durable commit (`Database::settle_now`).
    fn durable(&self) -> bool;
    fn finish(self: Box<Self>, commit_error: Option<Error>);
}

thread_local! {
    static WRITER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this thread is a database's writer.
pub(super) fn on_writer() -> bool {
    WRITER.with(std::cell::Cell::get)
}

struct Operation<T, F, C, R> {
    _permit: Admission,
    durable: bool,
    write: Option<F>,
    committed: C,
    reply: R,
    result: Option<Result<T>>,
}

pub(super) fn job<T, F, C, R>(permit: Admission, durable: bool, write: F, committed: C, reply: R) -> Job
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    C: FnOnce(&T) + Send + 'static,
    R: FnOnce(Result<T>) + Send + 'static,
{
    Box::new(Operation { _permit: permit, durable, write: Some(write), committed, reply, result: None })
}

impl<T, F, C, R> Work for Operation<T, F, C, R>
where
    T: Send,
    F: FnOnce(&Connection) -> Result<T> + Send,
    C: FnOnce(&T) + Send,
    R: FnOnce(Result<T>) + Send,
{
    fn apply(&mut self, connection: &Connection) -> bool {
        let write = self.write.take().expect("operation runs once");
        // One operation that panics fails alone: its savepoint is rolled back
        // and the writer goes on with the others.
        let result = catch_unwind(AssertUnwindSafe(|| write(connection)))
            .unwrap_or_else(|_| Err(Error::new("the database operation failed unexpectedly")));
        let success = result.is_ok();
        self.result = Some(result);
        success
    }
    fn failure(&self) -> Option<&Error> {
        self.result.as_ref().and_then(|result| result.as_ref().err())
    }
    fn refuse(&mut self, why: Error) {
        self.result = Some(Err(why));
    }
    fn durable(&self) -> bool {
        self.durable
    }
    fn finish(self: Box<Self>, commit_error: Option<Error>) {
        let Self { result, committed, reply, _permit, .. } = *self;
        let result = match result {
            Some(Err(error)) => Err(error),
            result => match commit_error {
                Some(error) => Err(error),
                None => result.expect("a committed operation was executed"),
            },
        };
        if let Ok(value) = &result
            && catch_unwind(AssertUnwindSafe(|| committed(value))).is_err()
        {
            // What a commit tells others is not the caller's to lose: the
            // operation stands, committed.
            eprintln!("a database commit's notification failed unexpectedly");
        }
        reply(result);
    }
}

pub(super) fn run(
    mut connection: Connection,
    queue: Arc<Queue>,
    metrics: Arc<Metrics>,
    damage: Arc<Damage>,
    path: PathBuf,
    unsettled: Arc<AtomicBool>,
    guard: Option<guard::Shared>,
) {
    fastctx_platform::roles::set(fastctx_platform::roles::Role::Blocking);
    WRITER.with(|writer| writer.set(true));
    // Off again as the writer ends, before its connection closes.
    let _hooked = guard.as_ref().map(|guard| guard::install(&connection, guard.clone()));
    // The writer waits on the disk: one pass the disk keeps longer than a
    // caller waits for its answer is stuck work of this process, told with
    // the rest (`fastctx_platform::doing`).
    let disk = format!("the disk, writing {}", path.file_name().unwrap_or_default().to_string_lossy());
    let mut dirty = true;
    // As the connection was opened; changed only between transactions.
    let mut full = true;
    let mut carried = None;
    loop {
        let Some(first) = carried.take().or_else(|| queue.pop(Duration::from_secs(1))) else {
            if dirty {
                let _disk = fastctx_platform::doing::waiting_outside(disk.clone(), super::ANSWER);
                // A busy reader may retain a WAL snapshot. Do not wait for it:
                // another idle pass retries the truncation.
                let _ = connection.busy_timeout(Duration::ZERO);
                let checkpoint = {
                    let _maintenance = crate::authorization::maintenance();
                    connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get::<_, i64>(0))
                };
                dirty = !matches!(checkpoint, Ok(0));
                let _ = connection.busy_timeout(super::ADMISSION);
            }
            metrics.sizes(
                std::fs::metadata(&path).map_or(0, |m| m.len()),
                std::fs::metadata(format!("{}-wal", path.display())).map_or(0, |m| m.len()),
            );
            if queue.finished() {
                return;
            }
            continue;
        };
        let durable = first.durable();
        let mut batch = vec![first];
        let waiting = fastctx_platform::doing::waiting_outside(disk.clone(), super::ANSWER);
        let result = synchronous(&connection, &mut full, durable).and_then(|()| {
            commit(&mut connection, &queue, &metrics, &damage, &mut batch, full, &mut carried, guard.as_ref())
        });
        if let Some(guard) = &guard {
            guard::lock(guard).committed(result.is_ok());
        }
        drop(waiting);
        dirty = true;
        match &result {
            Err(error) => damage.notice(error),
            Ok(()) if !full => unsettled.store(true, Ordering::Release),
            Ok(()) => {}
        }
        for job in batch {
            metrics.queue(-1);
            job.finish(result.as_ref().err().cloned());
        }
    }
}

/// Sets how far the next commit goes before it is answered: to the disk for
/// a durable batch, to the log alone for a relaxed one.
fn synchronous(connection: &Connection, full: &mut bool, durable: bool) -> Result<()> {
    if *full == durable {
        return Ok(());
    }
    let _maintenance = crate::authorization::maintenance();
    match connection.pragma_update(None, "synchronous", if durable { "FULL" } else { "NORMAL" }) {
        Ok(()) => {
            *full = durable;
            Ok(())
        }
        // A relaxed batch committed in full loses nothing.
        Err(_) if !durable => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn commit(
    connection: &mut Connection,
    queue: &Queue,
    metrics: &Metrics,
    damage: &Damage,
    batch: &mut Vec<Job>,
    full: bool,
    carried: &mut Option<Job>,
    guard: Option<&guard::Shared>,
) -> Result<()> {
    let _control = crate::authorization::transaction_control(true);
    let mut transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let started = Instant::now();
    loop {
        let job = batch.last_mut().expect("a nonempty batch");
        let mut savepoint = transaction.savepoint()?;
        let operation = Instant::now();
        guard::begin();
        let business = crate::authorization::transaction_control(false);
        let mut success = job.apply(&savepoint);
        if let Some(why) = guard::refused() {
            job.refuse(why);
            success = false;
        }
        if success && let Some(guard) = guard {
            if let Err(why) = guard::validate(guard, &savepoint) {
                job.refuse(why);
                success = false;
            }
            if let Some(why) = guard::refused() {
                job.refuse(why);
                success = false;
            }
        }
        drop(business);
        if success {
            savepoint.commit()?;
        } else {
            if let Some(error) = job.failure() {
                damage.notice(error);
            }
            savepoint.rollback()?;
        }
        if let Some(guard) = guard {
            let _business = crate::authorization::transaction_control(false);
            guard::lock(guard).ended(success);
        }
        metrics.operation(operation.elapsed(), !success);
        if batch.len() >= 32 || started.elapsed() >= Duration::from_millis(2) {
            break;
        }
        match queue.pop(Duration::ZERO) {
            // A durable operation is not committed relaxed: it begins the
            // next batch.
            Some(next) if !full && next.durable() => {
                *carried = Some(next);
                break;
            }
            Some(next) => batch.push(next),
            None => break,
        }
    }
    let flush = Instant::now();
    let result = transaction.commit();
    metrics.commit(flush.elapsed());
    result.map_err(|error| {
        // A commit that failed busy or for want of space left nothing behind;
        // any other may have reached the disk before it failed.
        let error = Error::from(error);
        match error.kind() {
            ErrorKind::Busy | ErrorKind::Full => error,
            _ => {
                damage.notice(&error);
                Error::with_kind(
                    ErrorKind::OutcomeUnknown,
                    format!("the database could not confirm its commit, which may or may not stand: {error}"),
                )
            }
        }
    })
}
