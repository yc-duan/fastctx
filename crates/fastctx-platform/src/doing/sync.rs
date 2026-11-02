//! Std mutexes of a place in the order of locks, for work that does not wait
//! on anything else. Two kinds, by what the holder does while it holds one:
//!
//! - `SyncMutex`: a thread takes it, does a few milliseconds of reading and
//!   writing its own records, and gives it back. Waiting for it blocks the
//!   thread, so a task thread waits for it in place (`crate::roles::in_place`)
//!   and a thread that keeps network lines alive does not take it.
//! - `MemoryMutex`: the holder only changes memory, for microseconds, as the
//!   hub's shared state is changed (`specs/world-engine.md`, 中枢). A task
//!   thread may take it; a thread that keeps network lines alive takes no
//!   lock of the work it carries.
//!
//! Either is over one unit, a conversation's or a record's (`new`), or over
//! many at once (`shared`): the machine's records, the hub's sessions. One
//! over many is held only briefly, as everything of every unit it is over
//! waits for it (`specs/world-foundation.md`, 宪法一): its holds are timed
//! together, and one held too long is a violation (`crate::conduct`).

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, PoisonError, TryLockError};

use super::Rank;
use super::registry::{self, Hold, Wait};
use crate::readings::Timing;

/// A std mutex that knows its place in the order of locks and tells the
/// registry who waits for it and who holds it, as the task's locks do: a
/// thread holding it takes only locks of later places, and threads that wait
/// on each other in a circle are found (`super::snapshot`).
struct Ranked<T: ?Sized> {
    id: u64,
    rank: Rank,
    name: &'static str,
    subject: Arc<str>,
    held: &'static Timing,
    shared: bool,
    inner: std::sync::Mutex<T>,
}

impl<T> Ranked<T> {
    fn new(rank: Rank, name: &'static str, subject: Arc<str>, shared: bool, value: T) -> Self {
        Self {
            id: registry::next(),
            rank,
            name,
            subject,
            held: registry::holds(name),
            shared,
            inner: std::sync::Mutex::new(value),
        }
    }
}

impl<T: ?Sized> Ranked<T> {
    /// Blocks this thread until the lock is taken. A holder that panicked
    /// leaves the lock to the next: one panic does not stop every later
    /// holder.
    fn take(&self) -> RankedGuard<'_, T> {
        let lock = registry::Lock {
            id: self.id,
            rank: self.rank,
            name: self.name,
            subject: &self.subject,
            held: self.held,
            shared: self.shared,
        };
        let wait = Wait::begin_blocking(lock);
        let guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        RankedGuard { guard, _hold: wait.held() }
    }

    /// The lock, if no one holds it now; told to the registry as `take`
    /// tells it.
    fn try_take(&self) -> Option<RankedGuard<'_, T>> {
        let guard = match self.inner.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        let lock = registry::Lock {
            id: self.id,
            rank: self.rank,
            name: self.name,
            subject: &self.subject,
            held: self.held,
            shared: self.shared,
        };
        Some(RankedGuard { guard, _hold: Wait::begin_blocking(lock).held() })
    }
}

/// A ranked mutex held for reading and writing records on disk.
pub struct SyncMutex<T: ?Sized>(Ranked<T>);

impl<T> SyncMutex<T> {
    /// A lock over one unit, `subject`.
    pub fn new(rank: Rank, name: &'static str, subject: impl Into<Arc<str>>, value: T) -> Self {
        Self(Ranked::new(rank, name, subject.into(), false, value))
    }

    /// A lock over many units at once.
    pub fn shared(rank: Rank, name: &'static str, value: T) -> Self {
        Self(Ranked::new(rank, name, name.into(), true, value))
    }
}

impl<T: ?Sized> SyncMutex<T> {
    /// Takes the lock; a free one at once, and one held by another after
    /// waiting for it in place (`crate::roles::in_place`), which a thread
    /// that keeps network lines alive must not do.
    #[track_caller]
    pub fn lock(&self) -> SyncMutexGuard<'_, T> {
        if let Some(guard) = self.0.try_take() {
            return guard;
        }
        crate::roles::in_place(self.0.name, || self.0.take())
    }
}

/// A ranked mutex held only to change memory.
pub struct MemoryMutex<T: ?Sized>(Ranked<T>);

impl<T> MemoryMutex<T> {
    /// A lock over one unit, `subject`.
    pub fn new(rank: Rank, name: &'static str, subject: impl Into<Arc<str>>, value: T) -> Self {
        Self(Ranked::new(rank, name, subject.into(), false, value))
    }

    /// A lock over many units at once.
    pub fn shared(rank: Rank, name: &'static str, value: T) -> Self {
        Self(Ranked::new(rank, name, name.into(), true, value))
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
