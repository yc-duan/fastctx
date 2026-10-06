//! The sessions this machine holds with other members (`crate::noise`).
//!
//! A machine starts a session with a member the first time it calls it,
//! and keeps it until either end forgets it: a node that restarts has none.
//! A call on a session the other end no longer has comes back unopened,
//! nothing of it run; the caller starts a new session and makes the call
//! again on it, so a restart on either side costs one handshake and is not
//! seen by whoever made the call.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fastctx_types::MachineName;

use crate::noise::{Keys, SessionId};

/// A session this machine starts with one member, behind the lock its
/// handshake holds.
type Slot = Arc<tokio::sync::Mutex<Option<Arc<Keys>>>>;

/// The sessions of one machine.
#[derive(Default)]
pub(crate) struct Sessions {
    /// The sessions this machine started, one per member. Each sits behind
    /// its own lock, so one handshake with a member runs at a time and the
    /// calls that wait for it take the session it makes.
    started: Mutex<HashMap<MachineName, Slot>>,
    /// The latest session each member started with this machine: a member
    /// starts another only when it lost the one before.
    answering: Mutex<HashMap<MachineName, Arc<Keys>>>,
}

impl Sessions {
    /// The slot for the session this machine starts with `with`.
    pub fn slot(&self, with: &MachineName) -> Slot {
        self.started.lock().expect("never poisoned").entry(with.clone()).or_default().clone()
    }

    /// Forgets the session this machine started with `with`, if it is
    /// still `session`: the other end does not have it any more.
    pub async fn forget(&self, with: &MachineName, session: SessionId) {
        let slot = self.slot(with);
        let mut slot = slot.lock().await;
        if slot.as_ref().is_some_and(|keys| keys.session == session) {
            *slot = None;
        }
    }

    /// Keeps the session `from` just started with this machine.
    pub fn answer(&self, from: &MachineName, keys: Keys) {
        self.answering.lock().expect("never poisoned").insert(from.clone(), Arc::new(keys));
    }

    /// The session `from` started with this machine, if it is `session`.
    pub fn answering(&self, from: &MachineName, session: SessionId) -> Option<Arc<Keys>> {
        self.answering.lock().expect("never poisoned").get(from).filter(|keys| keys.session == session).cloned()
    }

    /// Forgets every session with `name`, as when it is revoked.
    pub fn drop_member(&self, name: &MachineName) {
        self.started.lock().expect("never poisoned").remove(name);
        self.answering.lock().expect("never poisoned").remove(name);
    }
}
