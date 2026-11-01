//! Transient per-record gates for file migration and executor journals.
//! The registry lock covers only lookup; I/O holds only the selected record gate.

use fastctx_platform::doing::{Rank, SyncMutex};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

pub(crate) struct Gates {
    entries: Mutex<HashMap<PathBuf, Weak<SyncMutex<()>>>>,
    rank: Rank,
    name: &'static str,
}
impl Gates {
    pub(crate) fn new(rank: Rank, name: &'static str) -> Self {
        Self { entries: Mutex::new(HashMap::new()), rank, name }
    }
    pub(crate) fn get(&self, path: &Path) -> Gate<'_> {
        let mut entries = self.entries.lock().expect("record gates");
        let lock = entries.get(path).and_then(Weak::upgrade).unwrap_or_else(|| {
            let lock = Arc::new(SyncMutex::new(self.rank, self.name, path.display().to_string(), ()));
            entries.insert(path.to_path_buf(), Arc::downgrade(&lock));
            lock
        });
        Gate { gates: self, path: path.to_path_buf(), lock }
    }
}

pub(crate) struct Gate<'a> {
    gates: &'a Gates,
    path: PathBuf,
    lock: Arc<SyncMutex<()>>,
}
impl std::ops::Deref for Gate<'_> {
    type Target = SyncMutex<()>;
    fn deref(&self) -> &Self::Target {
        &self.lock
    }
}
impl Drop for Gate<'_> {
    fn drop(&mut self) {
        let mut entries = self.gates.entries.lock().expect("record gates");
        if Arc::strong_count(&self.lock) == 1 {
            entries.remove(&self.path);
        }
    }
}
