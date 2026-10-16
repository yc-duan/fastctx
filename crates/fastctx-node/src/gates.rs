//! One at a time for each path: what holds a path's gate goes on alone,
//! what comes for that path meanwhile waits, and other paths are not held.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use fastctx_platform::doing::{self, Rank};

pub(crate) struct Gates {
    rank: Rank,
    name: &'static str,
    gates: Mutex<HashMap<PathBuf, Weak<doing::Mutex<()>>>>,
}

impl Gates {
    /// Gates of the place `rank` in the order of locks, named `name`.
    pub(crate) fn new(rank: Rank, name: &'static str) -> Self {
        Self { rank, name, gates: Mutex::default() }
    }

    pub(crate) async fn lock(&self, path: &Path) -> Held {
        let gate = {
            let mut gates = self.gates.lock().expect("never poisoned");
            gates.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = gates.get(path).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(doing::Mutex::new(self.rank, self.name, path.display().to_string(), ()));
                gates.insert(path.to_path_buf(), Arc::downgrade(&gate));
                gate
            }
        };
        let guard = gate.lock_owned().await;
        Held { _guard: guard, _gate: gate }
    }
}

/// A path's gate held: the gate lives as long as it is held.
pub(crate) struct Held {
    _guard: doing::OwnedMutexGuard<()>,
    _gate: Arc<doing::Mutex<()>>,
}
