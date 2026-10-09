//! Words coming for residents of this machine, as a signal: a `run` call
//! waiting for its job answers when words come for its resident, so that
//! they reach it then rather than when the wait is over (`world-run.md`).

use std::collections::HashMap;
use std::sync::Mutex;

use fastctx_types::ResidentId;
use tokio::sync::watch;

#[derive(Default)]
pub struct Words {
    /// How many times words came for each resident since the node started.
    came: Mutex<HashMap<ResidentId, watch::Sender<u64>>>,
}

impl Words {
    /// Words for `resident` were kept, on their way into its session.
    pub fn came(&self, resident: &ResidentId) {
        self.sender(resident).send_modify(|count| *count += 1);
    }

    /// Changes whenever words come for `resident` from now on.
    pub fn watch(&self, resident: &ResidentId) -> watch::Receiver<u64> {
        self.sender(resident).subscribe()
    }

    fn sender(&self, resident: &ResidentId) -> watch::Sender<u64> {
        self.came.lock().expect("never poisoned").entry(resident.clone()).or_insert_with(|| watch::channel(0).0).clone()
    }
}
