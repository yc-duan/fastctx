//! The calls this machine is answering, each with what stops it: its caller
//! gave it up, the right it was made under was taken back, or the
//! connection it came on ended, so nobody is left to hear the answer. A
//! call is registered as it arrives, before its task starts, so a stop that
//! comes right behind it is never missed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::watch;

use super::{CallOn, Inner};

/// The calls being answered, by the connection each came on and the number
/// it came under there.
#[derive(Default)]
pub(crate) struct Answers {
    calls: Mutex<HashMap<(CallOn, u64), Entry>>,
    serial: AtomicU64,
}

struct Entry {
    serial: u64,
    stop: watch::Sender<Option<String>>,
}

/// A call while it is answered; dropping it takes it out of the register.
pub(crate) struct Answer {
    inner: Weak<Inner>,
    key: (CallOn, u64),
    serial: u64,
    stop: watch::Receiver<Option<String>>,
}

impl Answers {
    /// Registers the call `id` that came on `on`.
    pub(crate) fn open(&self, inner: &Arc<Inner>, on: CallOn, id: u64) -> Answer {
        let serial = self.serial.fetch_add(1, Ordering::Relaxed);
        let (stop, receiver) = watch::channel(None);
        let key = (on, id);
        self.calls.lock().expect("never poisoned").insert(key.clone(), Entry { serial, stop });
        Answer { inner: Arc::downgrade(inner), key, serial, stop: receiver }
    }

    /// Stops the call `id` that came on `on`, if it is still answered.
    pub(crate) fn stop(&self, on: &CallOn, id: u64, why: &str) {
        if let Some(entry) = self.calls.lock().expect("never poisoned").get(&(on.clone(), id)) {
            stop(entry, why);
        }
    }

    /// Stops every call that came on `on`, which ended.
    pub(crate) fn stop_all(&self, on: &CallOn, why: &str) {
        for (_, entry) in self.calls.lock().expect("never poisoned").iter().filter(|((came, _), _)| came == on) {
            stop(entry, why);
        }
    }
}

/// Tells a call why it stops, unless it was told already.
fn stop(entry: &Entry, why: &str) {
    entry.stop.send_if_modified(|stopped| {
        if stopped.is_some() {
            return false;
        }
        *stopped = Some(why.to_string());
        true
    });
}

impl Answer {
    /// Why the call is to stop, once it is.
    pub(crate) fn stopped(&self) -> Option<String> {
        self.stop.borrow().clone()
    }

    /// Resolves once the call is to stop, with why.
    pub(crate) async fn stopping(&mut self) -> String {
        loop {
            if let Some(why) = self.stop.borrow_and_update().clone() {
                return why;
            }
            // Its entry was replaced by another call under the same number
            // on the same connection: nothing stops this one any more.
            if self.stop.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        let Some(inner) = self.inner.upgrade() else { return };
        let mut calls = inner.answers.calls.lock().expect("never poisoned");
        if calls.get(&self.key).is_some_and(|entry| entry.serial == self.serial) {
            calls.remove(&self.key);
        }
    }
}
