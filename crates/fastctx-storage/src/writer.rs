//! A single writer batches ready operations without delaying an idle caller.

use super::{Error, Result};
use rusqlite::{Connection, TransactionBehavior};
use tokio::sync::{OwnedSemaphorePermit, mpsc};

pub(super) type Job = Box<dyn Work>;
pub(super) trait Work: Send {
    fn apply(&mut self, connection: &Connection) -> bool;
    fn finish(self: Box<Self>, commit_error: Option<Error>);
}

struct Operation<T, F, C, R> {
    _permit: OwnedSemaphorePermit,
    write: Option<F>,
    committed: C,
    reply: R,
    result: Option<Result<T>>,
}

pub(super) fn job<T, F, C, R>(permit: OwnedSemaphorePermit, write: F, committed: C, reply: R) -> Job
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
        let result = self.write.take().expect("operation runs once")(connection);
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
        if let Ok(value) = &result {
            committed(value);
        }
        reply(result);
    }
}

pub(super) fn run(mut connection: Connection, mut receive: mpsc::Receiver<Job>) {
    while let Some(first) = receive.blocking_recv() {
        let mut batch = vec![first];
        while batch.len() < 256 {
            match receive.try_recv() {
                Ok(job) => batch.push(job),
                Err(_) => break,
            }
        }
        let result = commit(&mut connection, &mut batch);
        for job in batch {
            job.finish(result.as_ref().err().cloned());
        }
    }
}

fn commit(connection: &mut Connection, batch: &mut [Job]) -> Result<()> {
    let mut transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for job in batch {
        let mut savepoint = transaction.savepoint()?;
        if job.apply(&savepoint) {
            savepoint.commit()?;
        } else {
            savepoint.rollback()?;
        }
    }
    transaction.commit()?;
    Ok(())
}
