//! Shared byte admission for bounded runtime working sets.
use crate::store::Error;
use std::sync::Arc;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError},
    time::Instant,
};

/// Clones share one fixed capacity. Queued multi-byte requests are FIFO,
/// including a large request ahead of smaller ones.
#[derive(Clone, Debug)]
pub struct ByteBudget {
    capacity: usize,
    semaphore: Arc<Semaphore>,
}
/// Retain this permit until the entire admitted working set has been released.
/// Dropping a waiting acquisition also returns its partial reservation.
#[derive(Debug)]
pub struct BytePermit {
    permit: OwnedSemaphorePermit,
}
impl ByteBudget {
    pub fn new(capacity: usize) -> Result<Self, Error> {
        if capacity == 0 || capacity > u32::MAX as usize || capacity > Semaphore::MAX_PERMITS {
            return Err(Error::Invalid("byte budget exceeds its counter range".into()));
        }
        Ok(Self { capacity, semaphore: Arc::new(Semaphore::new(capacity)) })
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    /// Includes partial reservations owned by queued requests, not just buffers
    /// whose allocation has already started.
    pub fn reserved(&self) -> usize {
        self.capacity - self.semaphore.available_permits()
    }
    /// Never waits. An impossible request returns Busy like a full pool.
    pub fn try_acquire(&self, bytes: usize) -> Result<BytePermit, Error> {
        if bytes > self.capacity {
            return Err(Error::Busy);
        }
        let permit = self.semaphore.clone().try_acquire_many_owned(bytes as u32).map_err(|error| match error {
            TryAcquireError::NoPermits => Error::Busy,
            TryAcquireError::Closed => Error::Unavailable("byte admission closed".into()),
        })?;
        Ok(BytePermit { permit })
    }
    /// Waits in FIFO order until the absolute deadline. Impossible requests
    /// never enter the queue; cancellation releases all reserved bytes.
    pub async fn acquire_until(&self, bytes: usize, deadline: Instant) -> Result<BytePermit, Error> {
        if bytes > self.capacity {
            return Err(Error::Invalid("byte request exceeds the pool capacity".into()));
        }
        if Instant::now() >= deadline {
            return Err(Error::Busy);
        }
        let permit = tokio::time::timeout_at(deadline, self.semaphore.clone().acquire_many_owned(bytes as u32))
            .await
            .map_err(|_| Error::Busy)?
            .map_err(|_| Error::Unavailable("byte admission closed".into()))?;
        // A ready permit must not win over an already expired deadline.
        if Instant::now() >= deadline {
            return Err(Error::Busy);
        }
        Ok(BytePermit { permit })
    }
    /// Prevents new admissions and wakes queued callers with Unavailable.
    /// Outstanding permits remain responsible for their working sets.
    pub fn close(&self) {
        self.semaphore.close();
    }
}
impl BytePermit {
    pub fn bytes(&self) -> usize {
        self.permit.num_permits()
    }
    /// Releases transient workspace after it has been freed, retaining the same
    /// admission for the buffers that remain alive. This never reacquires bytes.
    pub(crate) fn shrink_to(&mut self, bytes: usize) -> Result<(), Error> {
        if bytes > self.bytes() {
            return Err(Error::Invalid("a retained working set cannot grow its existing admission".into()));
        }
        let released = self.bytes() - bytes;
        drop(self.permit.split(released).expect("released bytes belong to this permit"));
        Ok(())
    }
}
