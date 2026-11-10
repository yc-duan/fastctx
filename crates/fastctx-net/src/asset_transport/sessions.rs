//! An asset session lives only as long as its authenticated control incarnation.
use crate::{sign::PublicKey, sync_transport::NegotiatedTransport};
use fastctx_sync::{
    admission::{ByteBudget, BytePermit},
    store::Error,
};
use fastctx_types::{
    MachineName,
    sync::session::{SessionBindingRequest, SessionId, SessionReady},
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::watch;

const ENTRY_BYTES: usize = 2048;
#[derive(Clone)]
pub struct Sessions(Arc<Registry>);
struct Registry {
    entries: Mutex<Entries>,
    budget: ByteBudget,
}
#[derive(Default)]
struct Entries {
    sessions: HashMap<SessionId, Entry>,
    controls: HashMap<(PublicKey, u64), HashSet<SessionId>>,
}
struct Entry {
    bound: Arc<BoundSession>,
    stop: watch::Sender<bool>,
}
/// Constructed by the authenticated control handler, never deserialized.
pub struct BoundSession {
    pub member: MachineName,
    pub key: PublicKey,
    pub generation: u64,
    pub binding: SessionBindingRequest,
    pub ready: SessionReady,
    pub selected: NegotiatedTransport,
    pub stopped: watch::Receiver<bool>,
    attached: AtomicBool,
    _permit: BytePermit,
}
/// One data socket consumes the logical session. Reconnect opens a new session
/// and fresh credit counters; only transfer identities survive reconnect.
pub struct Attachment(pub Arc<BoundSession>, Sessions);
impl Drop for Attachment {
    fn drop(&mut self) {
        self.1.close(self.0.key, self.0.generation, &self.0.ready.session);
    }
}
impl Sessions {
    pub fn new(budget: ByteBudget) -> Self {
        Self(Arc::new(Registry { entries: Mutex::new(Entries::default()), budget }))
    }
    pub fn budget(&self) -> &ByteBudget {
        &self.0.budget
    }
    pub fn open(
        &self,
        member: MachineName,
        key: PublicKey,
        generation: u64,
        binding: SessionBindingRequest,
        ready: SessionReady,
        selected: NegotiatedTransport,
    ) -> Result<(), Error> {
        ready.limits.validate().map_err(Error::Invalid)?;
        if &ready.limits != selected.limits() {
            return Err(Error::Invalid("asset readiness differs from the negotiated limits".into()));
        }
        match (&binding, &ready.holding) {
            (SessionBindingRequest::Hub { .. }, None) => {}
            (SessionBindingRequest::Object { object, .. }, Some(holding)) if object.scope == holding.scope => {}
            (SessionBindingRequest::Scope { scope }, Some(holding)) if scope == &holding.scope => {}
            _ => return Err(Error::Invalid("asset readiness differs from its session binding".into())),
        }
        // Serialized strings plus fixed struct and map slack remain charged until
        // the last socket or registry reference to the bound session is dropped.
        let retained = serde_json::to_vec(&(&member, &binding, &ready))
            .map_err(|error| Error::Invalid(error.to_string()))?
            .len()
            .checked_add(ENTRY_BYTES)
            .ok_or(Error::Busy)?;
        let mut entries = self.0.entries.lock().expect("never poisoned");
        let owner = (key, generation);
        if entries.sessions.contains_key(&ready.session) {
            return Err(Error::Invalid("asset session identifier is already registered".into()));
        }
        let share = self.0.budget.capacity() / 8 / ENTRY_BYTES;
        if entries.controls.get(&owner).is_some_and(|ids| ids.len() >= share) {
            return Err(Error::Busy);
        }
        let permit = self.0.budget.try_acquire(retained)?;
        let (stop, stopped) = watch::channel(false);
        let id = ready.session.clone();
        let bound = Arc::new(BoundSession {
            member,
            key,
            generation,
            binding,
            ready,
            selected,
            stopped,
            attached: AtomicBool::new(false),
            _permit: permit,
        });
        entries.sessions.insert(id.clone(), Entry { bound, stop });
        entries.controls.entry(owner).or_default().insert(id);
        Ok(())
    }
    pub fn get(&self, id: &SessionId) -> Option<Arc<BoundSession>> {
        self.0.entries.lock().expect("never poisoned").sessions.get(id).map(|e| e.bound.clone())
    }
    pub fn attach(&self, id: &SessionId, key: PublicKey) -> Result<Attachment, Error> {
        let bound = self
            .get(id)
            .filter(|s| s.key == key)
            .ok_or_else(|| Error::Denied("asset session is not this authenticated device's".into()))?;
        if *bound.stopped.borrow() || bound.stopped.has_changed().is_err() {
            return Err(Error::Unavailable("the asset control session ended".into()));
        }
        bound.attached.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).map_err(|_| Error::Busy)?;
        Ok(Attachment(bound, self.clone()))
    }
    pub fn close(&self, key: PublicKey, generation: u64, id: &SessionId) {
        let mut entries = self.0.entries.lock().expect("never poisoned");
        if entries.sessions.get(id).is_none_or(|e| e.bound.key != key || e.bound.generation != generation) {
            return;
        }
        if let Some(entry) = entries.sessions.remove(id) {
            entry.stop.send_replace(true);
        }
        if let Some(ids) = entries.controls.get_mut(&(key, generation)) {
            ids.remove(id);
            if ids.is_empty() {
                entries.controls.remove(&(key, generation));
            }
        }
    }
    pub fn close_control(&self, key: PublicKey, generation: u64) {
        let mut entries = self.0.entries.lock().expect("never poisoned");
        for id in entries.controls.remove(&(key, generation)).unwrap_or_default() {
            if let Some(entry) = entries.sessions.remove(&id) {
                entry.stop.send_replace(true);
            }
        }
    }
}
