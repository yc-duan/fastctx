//! A mutex of a place in the order of locks for work that does not wait on
//! anything else: a thread takes it, does a few milliseconds of reading and
//! writing its own records, and gives it back.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, PoisonError};

use super::Rank;
use super::registry::{self, Hold, Wait};

/// A std mutex that knows its place in the order of locks and tells the
/// registry who waits for it and who holds it, as the task's locks do: a
/// thread holding it takes only locks of later places, and threads that wait
/// on each other in a circle are found (`super::snapshot`).
pub struct SyncMutex<T: ?Sized> {
    id: u64,
    rank: Rank,
    name: &'static str,
    subject: Arc<str>,
    inner: std::sync::Mutex<T>,
}

impl<T> SyncMutex<T> {
    pub fn new(rank: Rank, name: &'static str, subject: impl Into<Arc<str>>, value: T) -> Self {
        Self { id: registry::next(), rank, name, subject: subject.into(), inner: std::sync::Mutex::new(value) }
    }
}

impl<T: ?Sized> SyncMutex<T> {
    /// Blocks this thread until the lock is taken. A holder that panicked
    /// leaves the lock to the next: what these locks guard is on disk, where
    /// a poisoned flag in memory protects nothing.
    pub fn lock(&self) -> SyncMutexGuard<'_, T> {
        let lock = registry::Lock { id: self.id, rank: self.rank, name: self.name, subject: &self.subject };
        let wait = Wait::begin_blocking(lock);
        let guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        SyncMutexGuard { guard, _hold: wait.held() }
    }
}

pub struct SyncMutexGuard<'a, T: ?Sized> {
    guard: std::sync::MutexGuard<'a, T>,
    _hold: Hold,
}

impl<T: ?Sized> Deref for SyncMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: ?Sized> DerefMut for SyncMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}
