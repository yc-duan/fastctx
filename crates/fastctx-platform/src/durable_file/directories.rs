//! Confirmation belongs to a process, not to the mere existence of a directory.
#[path = "confirmed.rs"]
mod confirmed;
use confirmed::Confirmed;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Component, Path},
    sync::{Arc, Condvar, LazyLock, Mutex, OnceLock},
};

const CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CONFIRMATIONS: usize = 128;
static BYTES: LazyLock<&'static crate::readings::Gauge> =
    LazyLock::new(|| crate::readings::gauge("sync.directory_cache.bytes"));
static INFLIGHT: LazyLock<&'static crate::readings::Gauge> =
    LazyLock::new(|| crate::readings::gauge("sync.directory_cache.confirmations"));
static EVICTIONS: LazyLock<&'static crate::readings::Counter> =
    LazyLock::new(|| crate::readings::counter("sync.directory_cache_evictions.ordinary"));

struct Cache {
    state: Mutex<State>,
    available: Condvar,
    limit: usize,
}
#[derive(Default)]
struct State {
    confirmed: Confirmed,
    inflight: BTreeMap<Arc<Path>, Arc<Entry>>,
    inflight_bytes: usize,
    reported_evictions: u64,
}
impl State {
    fn readings(&mut self) {
        BYTES.set((self.confirmed.bytes + self.inflight_bytes) as i64);
        INFLIGHT.set(self.inflight.len() as i64);
        EVICTIONS.add(self.confirmed.evictions - self.reported_evictions);
        self.reported_evictions = self.confirmed.evictions;
    }
}
impl Default for Cache {
    fn default() -> Self {
        Self { state: Mutex::default(), available: Condvar::new(), limit: CACHE_BYTES }
    }
}
#[derive(Default)]
struct Entry {
    result: Mutex<Option<bool>>,
    changed: Condvar,
}
impl Entry {
    fn wait(&self) -> bool {
        let mut result = self.result.lock().unwrap_or_else(|e| e.into_inner());
        while result.is_none() {
            result = self.changed.wait(result).unwrap_or_else(|e| e.into_inner());
        }
        result.expect("confirmation completed")
    }
}
struct Confirmation<'a> {
    cache: &'a Cache,
    path: Arc<Path>,
    entry: Arc<Entry>,
    prepared: bool,
}
impl Drop for Confirmation<'_> {
    fn drop(&mut self) {
        let mut state = self.cache.state.lock().unwrap_or_else(|e| e.into_inner());
        state.inflight.remove(self.path.as_ref());
        state.inflight_bytes -= charge(&self.path);
        if self.prepared {
            let available = self.cache.limit - state.inflight_bytes;
            state.confirmed.remember(&self.path, available);
        }
        state.readings();
        drop(state);
        *self.entry.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.prepared);
        self.entry.changed.notify_all();
        self.cache.available.notify_all();
    }
}
fn charge(path: &Path) -> usize {
    // Includes one path, the waiting entry, two Arc headers and tree allocation.
    1024 + path.as_os_str().len()
}
impl Cache {
    fn prepare(&self, path: &Path, before_confirm: &impl Fn(&Path)) -> io::Result<()> {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.confirmed.contains(path) {
                state.readings();
                return Ok(());
            }
        }
        // Never hold a child confirmation slot while waiting for its ancestry.
        if let Some(parent) = path.parent() {
            self.prepare(parent, before_confirm)?;
        }
        let bytes = charge(path);
        if bytes > self.limit {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "content path exceeds confirmation budget"));
        }
        let mut confirmation = loop {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.confirmed.contains(path) {
                state.readings();
                return Ok(());
            }
            if let Some(entry) = state.inflight.get(path).cloned() {
                drop(state);
                if entry.wait() {
                    return Ok(());
                }
                continue;
            }
            while state.confirmed.bytes + state.inflight_bytes + bytes > self.limit && state.confirmed.evict() {}
            state.readings();
            if state.inflight.len() >= MAX_CONFIRMATIONS || state.inflight_bytes + bytes > self.limit {
                drop(self.available.wait(state).unwrap_or_else(|e| e.into_inner()));
                continue;
            }
            let path: Arc<Path> = Arc::from(path);
            let entry = Arc::new(Entry::default());
            state.inflight.insert(path.clone(), entry.clone());
            state.inflight_bytes += bytes;
            state.readings();
            break Confirmation { cache: self, path, entry, prepared: false };
        };
        match fs::symlink_metadata(path) {
            Ok(metadata) => plain(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => super::create_directory(path)?,
            Err(error) => return Err(error),
        }
        before_confirm(path);
        if path.parent().is_some() {
            super::sync_parent(path)?;
        }
        confirmation.prepared = true;
        Ok(())
    }
}
pub(super) fn plain(metadata: &fs::Metadata) -> io::Result<()> {
    if metadata.is_dir() && !crate::owned_directory::is_link(metadata) {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "content path is not a plain directory"))
    }
}
pub(super) fn prepare(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::CurDir | Component::ParentDir)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "content directories require normalized absolute paths",
        ));
    }
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default).prepare(path, &|_| {})
}

#[cfg(test)]
#[path = "health.rs"]
mod health;
