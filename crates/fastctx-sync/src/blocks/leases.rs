//! Short in-memory admission protects IO without holding a mutex across it.

use crate::{identity::ScopeKey, manifest::BlobId, store::Error};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(super) struct Leases {
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    closed: BTreeSet<ScopeKey>,
    active: BTreeMap<BlobId, Active>,
}
#[derive(Default)]
struct Active {
    readers: u32,
    exclusive: bool,
}
pub(super) struct Lease {
    leases: Arc<Leases>,
    blob: BlobId,
    exclusive: bool,
}

impl Leases {
    pub(super) fn read(self: &Arc<Self>, blob: &BlobId) -> Result<Lease, Error> {
        self.acquire(blob, false, false)
    }
    pub(super) fn write(self: &Arc<Self>, blob: &BlobId) -> Result<Lease, Error> {
        self.acquire(blob, true, false)
    }
    pub(super) fn cleanup(self: &Arc<Self>, blob: &BlobId) -> Result<Lease, Error> {
        self.acquire(blob, true, true)
    }
    fn acquire(self: &Arc<Self>, blob: &BlobId, exclusive: bool, cleanup: bool) -> Result<Lease, Error> {
        let mut state = self.state.lock().expect("content lease state");
        if !cleanup && state.closed.contains(&blob.domain) {
            return Err(Error::Revoked);
        }
        let active = state.active.entry(blob.clone()).or_default();
        if active.exclusive || exclusive && active.readers != 0 {
            return Err(Error::Busy);
        }
        if exclusive {
            active.exclusive = true;
        } else {
            active.readers = active.readers.checked_add(1).ok_or(Error::Busy)?;
        }
        Ok(Lease { leases: self.clone(), blob: blob.clone(), exclusive })
    }
    /// Called only after the durable scope invalidation. A false result leaves
    /// cleanup pending; completion must wait for every old IO lease to drain.
    pub(super) fn close(&self, scope: &ScopeKey) -> bool {
        let mut state = self.state.lock().expect("content lease state");
        state.closed.insert(scope.clone());
        !state.active.keys().any(|blob| &blob.domain == scope)
    }
    /// A new publication generation is installed only after old work drained.
    pub(super) fn reopen(&self, scope: &ScopeKey) -> Result<(), Error> {
        let mut state = self.state.lock().expect("content lease state");
        if state.active.keys().any(|blob| &blob.domain == scope) {
            return Err(Error::Busy);
        }
        state.closed.remove(scope);
        Ok(())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.leases.state.lock().expect("content lease state");
        let active = state.active.get_mut(&self.blob).expect("a live content lease is registered");
        if self.exclusive {
            active.exclusive = false;
        } else {
            active.readers -= 1;
        }
        if !active.exclusive && active.readers == 0 {
            state.active.remove(&self.blob);
        }
    }
}
