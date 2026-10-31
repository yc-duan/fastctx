//! Std mutexes of a place in the order of locks, for work that does not wait
//! on anything else. Two kinds, by what the holder does while it holds one:
//!
//! - `SyncMutex`: a thread takes it, does a few milliseconds of reading and
//!   writing its own records, and gives it back. It blocks its thread, so it
//!   is taken only on threads that may block.
//! - `MemoryMutex`: the holder only changes memory, for microseconds, as the
//!   hub's shared state is changed (`specs/world-engine.md`, 中枢). A task
//!   thread may take it; a thread that keeps network lines alive takes no
//!   lock of the work it carries.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, PoisonError};

use super::Rank;
use super::registry::{self, Hold, Wait};

/// A std mutex that knows its place in the order of locks and tells the
/// registry who waits for it and who holds it, as the task's locks do: a
/// thread holding it takes only locks of later places, and threads that wait
/// on each other in a circle are found (`super::snapshot`).
struct Ranked<T: ?Sized> {
    id: u64,
    rank: Rank,
    name: &'static str,
    subject: Arc<str>,
    inner: std::sync::Mutex<T>,
}

impl<T> Ranked<T> {
    fn new(rank: Rank, name: &'static str, subject: Arc<str>, value: T) -> Self {
        Self { id: registry::next(), rank, name, subject, inner: std::sync::Mutex::new(value) }
    }
}

impl<T: ?Sized> Ranked<T> {
    /// Blocks this thread until the lock is taken. A holder that panicked
    /// leaves the lock to the next: one panic does not stop every later
    /// holder.
    fn take(&self) -> RankedGuard<'_, T> {
        let lock = registry::Lock { id: self.id, rank: self.rank, name: self.name, subject: &self.subject };
        let wait = Wait::begin_blocking(lock);
        let guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        RankedGuard { guard, _hold: wait.held() }
    }
}

/// A ranked mutex held for reading and writing records on disk.
pub struct SyncMutex<T: ?Sized>(Ranked<T>);

impl<T> SyncMutex<T> {
    pub fn new(rank: Rank, name: &'static str, subject: impl Into<Arc<str>>, value: T) -> Self {
        Self(Ranked::new(rank, name, subject.into(), value))
    }
}

impl<T: ?Sized> SyncMutex<T> {
    /// Blocks this thread until the lock is taken. Taken on a thread that
    /// must not block, it is noted as a violation (`crate::roles`).
    #[track_caller]
    pub fn lock(&self) -> SyncMutexGuard<'_, T> {
        crate::roles::blocking(self.0.name);
        self.0.take()
    }
}

/// A ranked mutex held only to change memory.
pub struct MemoryMutex<T: ?Sized>(Ranked<T>);

impl<T> MemoryMutex<T> {
    pub fn new(rank: Rank, name: &'static str, subject: impl Into<Arc<str>>, value: T) -> Self {
        Self(Ranked::new(rank, name, subject.into(), value))
    }
}

impl<T: ?Sized> MemoryMutex<T> {
    /// Takes the lock, waiting the moment another holder needs. Taken on a
    /// thread that keeps network lines alive, it is noted as a violation
    /// (`crate::roles`).
    #[track_caller]
    pub fn lock(&self) -> MemoryMutexGuard<'_, T> {
        crate::roles::locking(self.0.name);
        self.0.take()
    }
}

pub struct RankedGuard<'a, T: ?Sized> {
    guard: std::sync::MutexGuard<'a, T>,
    _hold: Hold,
}

pub type SyncMutexGuard<'a, T> = RankedGuard<'a, T>;
pub type MemoryMutexGuard<'a, T> = RankedGuard<'a, T>;

impl<T: ?Sized> Deref for RankedGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: ?Sized> DerefMut for RankedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}
