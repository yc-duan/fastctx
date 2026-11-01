//! A single writer batches ready operations without delaying an idle caller.

use super::{Error, ErrorKind, Result};
use crate::{
    integrity::Damage,
    metrics::Metrics,
    queue::{Admission, Queue},
};
use rusqlite::{Connection, TransactionBehavior};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) type Job = Box<dyn Work>;
pub(super) trait Work: Send {
    fn apply(&mut self, connection: &Connection) -> bool;
    /// Why the operation failed, once applied.
    fn failure(&self) -> Option<&Error>;
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
    write: Option<F>,
    committed: C,
    reply: R,
    result: Option<Result<T>>,
}

pub(super) fn job<T, F, C, R>(permit: Admission, write: F, committed: C, reply: R) -> Job
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    C: FnOnce(&T) + Send + 'static,
    R: FnOnce(Result<T>) + Send + 'static,
{
    Box::new(Operation { _permit: permit, write: Some(write), committed, reply, result: None })
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
) {
    fastctx_platform::roles::set(fastctx_platform::roles::Role::Blocking);
    WRITER.with(|writer| writer.set(true));
    let mut dirty = true;
    loop {
        let Some(first) = queue.pop(Duration::from_secs(1)) else {
            if dirty {
                // A busy reader may retain a WAL snapshot. Do not wait for it:
                // another idle pass retries the truncation.
                let _ = connection.busy_timeout(Duration::ZERO);
                let checkpoint =
                    connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get::<_, i64>(0));
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
        let mut batch = vec![first];
        let result = commit(&mut connection, &queue, &metrics, &damage, &mut batch);
        dirty = true;
        if let Err(error) = &result {
            damage.notice(error);
        }
        for job in batch {
            metrics.queue(-1);
            job.finish(result.as_ref().err().cloned());
        }
    }
}

fn commit(
    connection: &mut Connection,
    queue: &Queue,
    metrics: &Metrics,
    damage: &Damage,
    batch: &mut Vec<Job>,
) -> Result<()> {
    let mut transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let started = Instant::now();
    loop {
        let job = batch.last_mut().expect("a nonempty batch");
        let mut savepoint = transaction.savepoint()?;
        let operation = Instant::now();
        let success = job.apply(&savepoint);
        if success {
            savepoint.commit()?;
        } else {
            if let Some(error) = job.failure() {
                damage.notice(error);
            }
            savepoint.rollback()?;
        }
        metrics.operation(operation.elapsed(), !success);
        if batch.len() >= 32 || started.elapsed() >= Duration::from_millis(2) {
            break;
        }
        match queue.pop(Duration::ZERO) {
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
