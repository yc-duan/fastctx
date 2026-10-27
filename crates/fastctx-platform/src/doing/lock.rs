//! Tokio's mutex and read-write lock, each knowing its place in the order of
//! locks and telling the registry who waits for it and who holds it.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Duration;

use super::Rank;
use super::registry::{self, Hold, Wait};

struct Identity {
    id: u64,
    rank: Rank,
    name: &'static str,
    subject: Arc<str>,
}

impl Identity {
    fn new(rank: Rank, name: &'static str, subject: Arc<str>) -> Self {
        Self { id: registry::next(), rank, name, subject }
    }

    fn lock(&self) -> registry::Lock<'_> {
        registry::Lock { id: self.id, rank: self.rank, name: self.name, subject: &self.subject }
    }
}

/// A mutex of a place in the order of locks: `name` says which lock it is,
/// `subject` whose, as in `speaking(claude-26bf)`.
pub struct Mutex<T: ?Sized> {
    identity: Identity,
    inner: Arc<tokio::sync::Mutex<T>>,
}

impl<T> Mutex<T> {
    pub fn new(rank: Rank, name: &'static str, subject: impl Into<Arc<str>>, value: T) -> Self {
        Self { identity: Identity::new(rank, name, subject.into()), inner: Arc::new(tokio::sync::Mutex::new(value)) }
    }
}

impl<T: ?Sized> Mutex<T> {
    pub async fn lock(&self) -> MutexGuard<'_, T> {
        let wait = Wait::begin(self.identity.lock(), true);
        let guard = self.inner.lock().await;
        MutexGuard { guard, _hold: wait.held() }
    }

    /// As `lock`, waiting at most `within`.
    pub async fn lock_within(&self, within: Duration) -> Result<MutexGuard<'_, T>, Busy> {
        tokio::time::timeout(within, self.lock())
            .await
            .map_err(|_| Busy { doing: registry::keeping(self.identity.id).filter(|label| super::said(label)) })
    }

    pub fn try_lock(&self) -> Result<MutexGuard<'_, T>, tokio::sync::TryLockError> {
        let guard = self.inner.try_lock()?;
        Ok(MutexGuard { guard, _hold: registry::taken(self.identity.lock(), true) })
    }

    /// As `lock`, held by a guard that owns its share of the mutex, so it
    /// can outlive the borrow it was taken from.
    pub async fn lock_owned(&self) -> OwnedMutexGuard<T> {
        let wait = Wait::begin(self.identity.lock(), true);
        let guard = self.inner.clone().lock_owned().await;
        OwnedMutexGuard { guard, _hold: wait.held() }
    }
}

pub struct MutexGuard<'a, T: ?Sized> {
    guard: tokio::sync::MutexGuard<'a, T>,
    _hold: Hold,
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

pub struct OwnedMutexGuard<T: ?Sized> {
    guard: tokio::sync::OwnedMutexGuard<T>,
    _hold: Hold,
}

impl<T: ?Sized> OwnedMutexGuard<T> {
    /// Attributes this owned guard to the task it was moved into, so waits
    /// and lock-order diagnostics follow the task doing the protected work.
    pub fn adopt(&self) {
        self._hold.adopt();
    }
}

impl<T: ?Sized> Deref for OwnedMutexGuard<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: ?Sized> DerefMut for OwnedMutexGuard<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

/// A read-write lock of a place in the order of locks. Tokio's is fair: once
/// a writer waits, readers that come after it wait for it too.
pub struct RwLock<T: ?Sized> {
    identity: Identity,
    inner: Arc<tokio::sync::RwLock<T>>,
}

impl<T> RwLock<T> {
    pub fn new(rank: Rank, name: &'static str, subject: impl Into<Arc<str>>, value: T) -> Self {
        Self { identity: Identity::new(rank, name, subject.into()), inner: Arc::new(tokio::sync::RwLock::new(value)) }
    }
}

impl<T: ?Sized> RwLock<T> {
    pub async fn read(&self) -> RwLockReadGuard<'_, T> {
        let wait = Wait::begin(self.identity.lock(), false);
        let guard = self.inner.read().await;
        RwLockReadGuard { guard, _hold: wait.held() }
    }

    pub async fn write(&self) -> RwLockWriteGuard<'_, T> {
        let wait = Wait::begin(self.identity.lock(), true);
        let guard = self.inner.write().await;
        RwLockWriteGuard { guard, _hold: wait.held() }
    }

    /// As `read`, held by a guard that owns its share of the lock.
    pub async fn read_owned(&self) -> OwnedRwLockReadGuard<T> {
        let wait = Wait::begin(self.identity.lock(), false);
        let guard = self.inner.clone().read_owned().await;
        OwnedRwLockReadGuard { _guard: guard, _hold: wait.held() }
    }

    /// As `write`, held by a guard that owns its share of the lock.
    pub async fn write_owned(&self) -> OwnedRwLockWriteGuard<T> {
        let wait = Wait::begin(self.identity.lock(), true);
        let guard = self.inner.clone().write_owned().await;
        OwnedRwLockWriteGuard { _guard: guard, _hold: wait.held() }
    }

    /// As `read`, waiting at most `within`.
    pub async fn read_within(&self, within: Duration) -> Result<RwLockReadGuard<'_, T>, Busy> {
        tokio::time::timeout(within, self.read()).await.map_err(|_| self.busy())
    }

    /// As `write`, waiting at most `within`.
    pub async fn write_within(&self, within: Duration) -> Result<RwLockWriteGuard<'_, T>, Busy> {
        tokio::time::timeout(within, self.write()).await.map_err(|_| self.busy())
    }

    fn busy(&self) -> Busy {
        Busy { doing: registry::keeping(self.identity.id).filter(|label| super::said(label)) }
    }

    pub fn try_write(&self) -> Result<RwLockWriteGuard<'_, T>, tokio::sync::TryLockError> {
        let guard = self.inner.try_write()?;
        Ok(RwLockWriteGuard { guard, _hold: registry::taken(self.identity.lock(), true) })
    }
}

/// A lock not taken in the time it was waited for.
#[derive(Clone, Debug)]
pub struct Busy {
    /// What the task keeping it was doing then, as it said (`super::doing`).
    pub doing: Option<Arc<str>>,
}

/// A share of a read-write lock held to read, owning its share of the lock.
pub struct OwnedRwLockReadGuard<T: ?Sized> {
    _guard: tokio::sync::OwnedRwLockReadGuard<T>,
    _hold: Hold,
}

/// A read-write lock held alone, owning its share of the lock.
pub struct OwnedRwLockWriteGuard<T: ?Sized> {
    _guard: tokio::sync::OwnedRwLockWriteGuard<T>,
    _hold: Hold,
}

pub struct RwLockReadGuard<'a, T: ?Sized> {
    guard: tokio::sync::RwLockReadGuard<'a, T>,
    _hold: Hold,
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

pub struct RwLockWriteGuard<'a, T: ?Sized> {
    guard: tokio::sync::RwLockWriteGuard<'a, T>,
    _hold: Hold,
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}
