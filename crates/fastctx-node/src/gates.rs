//! One at a time for each path: what holds a path's gate goes on alone,
//! what comes for that path meanwhile waits, and other paths are not held.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

#[derive(Default)]
pub(crate) struct Gates {
    gates: Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
}

impl Gates {
    pub(crate) async fn lock(&self, path: &Path) -> tokio::sync::OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self.gates.lock().expect("never poisoned");
            gates.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = gates.get(path).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(path.to_path_buf(), Arc::downgrade(&gate));
                gate
            }
        };
        gate.lock_owned().await
    }
}
