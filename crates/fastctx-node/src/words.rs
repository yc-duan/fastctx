//! Words coming for residents, as a signal: a `run` call waiting for its job
//! answers when words come for its resident, so that they reach it then
//! rather than when the wait is over (`world-run.md`). A resident's words
//! come where it lives; a run of it on the machine it works on hears of them
//! from there (`RemoteCall::WordsCame`).

use fastctx_types::Address;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Default)]
pub struct Words {
    came: Arc<Mutex<HashMap<Address, watch::Sender<u64>>>>,
}

impl Words {
    /// Words for `resident` were kept, on their way into its session.
    pub fn came(&self, resident: &Address) {
        if let Some(sender) = self.came.lock().expect("words poisoned").get(resident) {
            sender.send_modify(|count| *count += 1);
        }
    }

    /// Changes whenever words come for `resident` from now on.
    pub fn watch(&self, resident: &Address) -> watch::Receiver<u64> {
        let mut entries = self.came.lock().expect("words poisoned");
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
                let mut entries = registry.lock().expect("words poisoned");
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
