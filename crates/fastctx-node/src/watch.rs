//! Watching residents live: every event of a session the World hears from,
//! handed to each watcher as it happens. A watcher that goes away is
//! dropped the next time there is something to hand it.

use std::collections::HashMap;
use std::sync::Mutex;

use fastctx_types::ResidentId;
use fastctx_types::session::SessionEvent;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct Watchers {
    watching: Mutex<HashMap<ResidentId, Vec<mpsc::UnboundedSender<SessionEvent>>>>,
}

impl Watchers {
    /// Everything that happens in the resident's session from now on.
    pub fn watch(&self, id: &ResidentId) -> mpsc::UnboundedReceiver<SessionEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.watching.lock().expect("never poisoned").entry(id.clone()).or_default().push(sender);
        receiver
    }

    pub fn publish(&self, id: &ResidentId, event: &SessionEvent) {
        let mut watching = self.watching.lock().expect("never poisoned");
        if let Some(watchers) = watching.get_mut(id) {
            watchers.retain(|watcher| watcher.send(event.clone()).is_ok());
            if watchers.is_empty() {
                watching.remove(id);
            }
        }
    }
}
