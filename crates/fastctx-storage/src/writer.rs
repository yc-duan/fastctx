//! A single writer batches ready operations without delaying an idle caller.

use super::{Error, Result};
use crate::{
    metrics::Metrics,
    queue::{Admission, Queue},
};
use rusqlite::{Connection, TransactionBehavior};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, atomic::Ordering};
use std::time::{Duration, Instant};

pub(super) type Job = Box<dyn Work>;
pub(super) trait Work: Send {
    fn apply(&mut self, connection: &Connection) -> bool;
    fn finish(self: Box<Self>, commit_error: Option<Error>);
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

pub(super) fn run(mut connection: Connection, queue: Arc<Queue>, metrics: Arc<Metrics>, path: PathBuf) {
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
            metrics.database_bytes.store(std::fs::metadata(&path).map_or(0, |m| m.len()), Ordering::Relaxed);
            metrics
                .wal_bytes
                .store(std::fs::metadata(format!("{}-wal", path.display())).map_or(0, |m| m.len()), Ordering::Relaxed);
            if queue.finished() {
                return;
            }
            continue;
        };
        let mut batch = vec![first];
        let result = commit(&mut connection, &queue, &metrics, &mut batch);
        dirty = true;
        for job in batch {
            metrics.queued.fetch_sub(1, Ordering::Relaxed);
            job.finish(result.as_ref().err().cloned());
        }
    }
}

fn commit(connection: &mut Connection, queue: &Queue, metrics: &Metrics, batch: &mut Vec<Job>) -> Result<()> {
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
            metrics.failures.fetch_add(1, Ordering::Relaxed);
            savepoint.rollback()?;
        }
        let elapsed = operation.elapsed();
        metrics.write_micros.record(elapsed);
        metrics.operations.fetch_add(1, Ordering::Relaxed);
        if elapsed > Duration::from_millis(20) {
            metrics.slow_operations.fetch_add(1, Ordering::Relaxed);
        }
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
    metrics.commit_micros.record(flush.elapsed());
    metrics.commits.fetch_add(1, Ordering::Relaxed);
    result.map_err(Into::into)
}
