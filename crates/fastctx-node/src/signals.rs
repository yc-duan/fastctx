//! Signals for residents, which the `run` calls waiting for their jobs
//! listen for: words came, so a call answers then rather than when its wait
//! is over; or the turn was interrupted, so the job it waits for stops with
//! it (`world-run.md`). A resident's signal is raised where it lives; a run
//! of it on the machine it works on hears of it from there
//! (`RemoteCall::WordsCame`, `RemoteCall::TurnInterrupted`).

use fastctx_types::Address;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Default)]
pub struct Signals {
    came: Arc<Mutex<HashMap<Address, watch::Sender<u64>>>>,
}

impl Signals {
    /// The signal is raised for `resident`.
    pub fn raise(&self, resident: &Address) {
        if let Some(sender) = self.came.lock().expect("signals poisoned").get(resident) {
            sender.send_modify(|count| *count += 1);
        }
    }

    /// Changes whenever the signal is raised for `resident` from now on.
    pub fn watch(&self, resident: &Address) -> watch::Receiver<u64> {
        let mut entries = self.came.lock().expect("signals poisoned");
        if let Some(sender) = entries.get(resident) {
            return sender.subscribe();
        }
        let (sender, receiver) = watch::channel(0);
        entries.insert(resident.clone(), sender.clone());
        let (registry, resident) = (Arc::downgrade(&self.came), resident.clone());
        tokio::spawn(async move {
            loop {
                sender.closed().await;
                let Some(registry) = registry.upgrade() else { return };
                let mut entries = registry.lock().expect("signals poisoned");
                // Subscription and retirement share the registry lock.
                if sender.receiver_count() == 0 {
                    entries.remove(&resident);
                    if entries.is_empty() {
                        *entries = HashMap::new();
                    }
                    return;
                }
            }
        });
        receiver
    }
}
