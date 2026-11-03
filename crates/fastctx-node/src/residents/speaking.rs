//! A speech worker retains the same permit after its initiating future is
//! cancelled. The registry lease stays alive until the actual work ends.
use fastctx_platform::doing::{Mutex, MutexGuard, OwnedMutexGuard};
use fastctx_types::ResidentId;
use std::{
    io,
    sync::{Arc, Mutex as MemoryMutex, Weak},
};

pub struct Speaking {
    mutex: Mutex<()>,
    active: MemoryMutex<Weak<OwnedMutexGuard<()>>>,
}
impl Speaking {
    pub(super) fn new(id: &ResidentId) -> Self {
        Self { mutex: crate::ranked::speaking(id), active: MemoryMutex::new(Weak::new()) }
    }
}
pub struct Lease(crate::leases::Lease<ResidentId, Speaking>);
impl Lease {
    pub(super) fn new(lease: crate::leases::Lease<ResidentId, Speaking>) -> Self {
        Self(lease)
    }
    pub async fn lock(&self) -> Arc<Held> {
        let permit = Arc::new(self.0.mutex.lock_owned().await);
        *self.0.active.lock().expect("speech permit") = Arc::downgrade(&permit);
        Arc::new(Held { _permit: permit, _lease: self.0.clone() })
    }
    pub async fn lock_owned(&self) -> Arc<Held> {
        self.lock().await
    }
    pub fn try_lock(&self) -> Result<MutexGuard<'_, ()>, tokio::sync::TryLockError> {
        self.0.mutex.try_lock()
    }
    /// As `lock`, at once or not at all.
    pub fn try_held(&self) -> Option<Arc<Held>> {
        let permit = Arc::new(self.0.mutex.try_lock_owned().ok()?);
        *self.0.active.lock().expect("speech permit") = Arc::downgrade(&permit);
        Some(Arc::new(Held { _permit: permit, _lease: self.0.clone() }))
    }
}

pub struct Held {
    _permit: Arc<OwnedMutexGuard<()>>,
    _lease: crate::leases::Lease<ResidentId, Speaking>,
}
impl Held {
    /// The caller already owns this resident's speech permit.
    pub(super) fn new(lease: crate::leases::Lease<ResidentId, Speaking>) -> io::Result<Arc<Self>> {
        let permit = lease
            .active
            .lock()
            .expect("speech permit")
            .upgrade()
            .ok_or_else(|| io::Error::other("speech worker started without its permit"))?;
        Ok(Arc::new(Self { _permit: permit, _lease: lease }))
    }
}

impl std::ops::Deref for Lease {
    type Target = Speaking;
    fn deref(&self) -> &Speaking {
        &self.0
    }
}
