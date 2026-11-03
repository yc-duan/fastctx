//! Dormant host projections and terminal identities, kept small. A thread
//! with nothing under way is kept as its encoded bytes; only past a bound
//! of bytes are the oldest of them put aside on disk, by a thread of their
//! own and never under the host's lock (`fastctx_platform::aside`). Nothing
//! here ends the node: a projection that cannot be put aside stays in
//! memory, and one that cannot be read back is taken as unknown, and said.

use super::Thread;
use fastctx_platform::{aside::Aside, ensure_private_dir};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque, hash_map};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The bytes of dormant threads kept in memory before the oldest are put
/// aside on disk: hundreds of conversations' threads, at their usual size.
const DORMANT_BYTES: usize = 32 << 20;

/// The turns whose end is remembered, the oldest forgotten first: far more
/// than are ever reported twice, which happens within moments.
const FINISHED: usize = 1 << 16;

pub(super) fn fresh(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    // A node before this one may still be letting its files go. A
    // directory that cannot be reset only keeps more in memory.
    if let Err(error) = fastctx_platform::owned_directory::remove_tree(&path).and_then(|()| ensure_private_dir(&path)) {
        eprintln!("fastctx node: resetting {}: {error}; dormant Codex threads stay in memory", path.display());
    }
    path
}

pub(super) fn key(value: &impl serde::Serialize) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).expect("host identity serializes")))
}

pub(super) struct Threads {
    directory: PathBuf,
    generation: u64,
    entries: HashMap<String, Thread>,
    /// Dormant threads kept in memory, encoded, oldest first.
    dormant: HashMap<String, Vec<u8>>,
    order: VecDeque<String>,
    dormant_bytes: usize,
    /// Dormant threads put aside on disk, or on their way there.
    aside: HashSet<String>,
    putting: Aside,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    generation: u64,
    thread: Thread,
}

impl Threads {
    pub fn new(root: &Path) -> Self {
        Self {
            directory: fresh(root, "runtime-threads"),
            generation: 0,
            entries: HashMap::new(),
            dormant: HashMap::new(),
            order: VecDeque::new(),
            dormant_bytes: 0,
            aside: HashSet::new(),
            putting: Aside::new("codex-thread-cache", "a dormant Codex thread"),
        }
    }
    pub fn restarted(&mut self) {
        self.generation += 1;
    }
    fn path(&self, thread: &str) -> PathBuf {
        self.directory.join(format!("{}.json", key(&thread)))
    }
    fn load(&mut self, thread: &str) {
        if self.entries.contains_key(thread) {
            return;
        }
        let bytes = if let Some(bytes) = self.dormant.remove(thread) {
            self.dormant_bytes -= bytes.len();
            self.order.retain(|kept| kept != thread);
            bytes
        } else if self.aside.contains(thread) {
            let path = self.path(thread);
            // Only threads past the bound in memory are read back from disk.
            match self.putting.on_its_way(&path).map_or_else(|| fastctx_platform::disk::read(&path), Ok) {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("fastctx node: reading a dormant Codex thread back: {error}; it is taken as unknown");
                    return;
                }
            }
        } else {
            return;
        };
        let mut stored: Stored = match serde_json::from_slice(&bytes) {
            Ok(stored) => stored,
            Err(error) => {
                eprintln!("fastctx node: decoding a dormant Codex thread: {error}; it is taken as unknown");
                return;
            }
        };
        if stored.generation != self.generation {
            stored.thread.usage = None;
        }
        self.entries.insert(thread.into(), stored.thread);
    }
    pub fn entry(&mut self, thread: String) -> hash_map::Entry<'_, String, Thread> {
        self.load(&thread);
        self.entries.entry(thread)
    }
    pub fn get(&mut self, thread: &str) -> Option<&Thread> {
        self.load(thread);
        self.entries.get(thread)
    }
    pub fn get_mut(&mut self, thread: &str) -> Option<&mut Thread> {
        self.load(thread);
        self.entries.get_mut(thread)
    }
    pub fn values(&self) -> hash_map::Values<'_, String, Thread> {
        self.entries.values()
    }
    /// Forgets a thread that no longer exists, here and on disk.
    pub fn remove(&mut self, thread: &str) {
        self.entries.remove(thread);
        if let Some(bytes) = self.dormant.remove(thread) {
            self.dormant_bytes -= bytes.len();
            self.order.retain(|kept| kept != thread);
        }
        if self.aside.remove(thread) {
            self.putting.remove(self.path(thread));
        }
    }
    pub fn keys(&self) -> hash_map::Keys<'_, String, Thread> {
        self.entries.keys()
    }
    /// Keeps the threads with nothing under way encoded, the oldest put
    /// aside on disk past the bound; the disk is never waited for here.
    pub fn release(&mut self) {
        let dormant: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                !entry.loaded
                    && !entry.submitted
                    && entry.turn.is_none()
                    && entry.unsettled.is_empty()
                    && entry.requests.is_empty()
                    && entry.items.is_empty()
                    && entry.item_starts.is_empty()
                    && entry.started.is_empty()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in dormant {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "generation": self.generation, "thread": self.entries[&id],
            }))
            .expect("encoding Codex thread cache");
            self.entries.remove(&id);
            self.dormant_bytes += bytes.len();
            self.dormant.insert(id.clone(), bytes);
            self.order.push_back(id);
        }
        if self.entries.is_empty() {
            self.entries = HashMap::new();
        }
        while self.dormant_bytes > DORMANT_BYTES {
            let Some(oldest) = self.order.pop_front() else { break };
            let Some(bytes) = self.dormant.remove(&oldest) else { continue };
            self.dormant_bytes -= bytes.len();
            self.putting.put(self.path(&oldest), bytes);
            self.aside.insert(oldest);
        }
    }
}

impl<'a> IntoIterator for &'a mut Threads {
    type Item = (&'a String, &'a mut Thread);
    type IntoIter = hash_map::IterMut<'a, String, Thread>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter_mut()
    }
}

/// A turn's identity, as its thread and its own name digested.
type TurnKey = [u8; 16];

/// The turns whose end was reported, the latest `FINISHED` of them.
pub(super) struct Finished(Mutex<(HashSet<TurnKey>, VecDeque<TurnKey>)>);
impl Finished {
    /// Remembered in memory; the directory an earlier node kept them in goes.
    pub fn new(root: &Path) -> Self {
        let _ = fastctx_platform::owned_directory::remove_tree(&root.join("runtime-finished"));
        Self(Mutex::new((HashSet::new(), VecDeque::new())))
    }
    fn key(thread: &str, turn: &str) -> TurnKey {
        let digest = Sha256::digest(serde_json::to_vec(&(thread, turn)).expect("turn identity serializes"));
        digest[..16].try_into().expect("a digest is longer than 16 bytes")
    }
    pub fn contains(&self, thread: &str, turn: &str) -> bool {
        self.0.lock().expect("never poisoned").0.contains(&Self::key(thread, turn))
    }
    /// Whether the end of `turn` was not reported before.
    pub fn insert(&self, thread: &str, turn: &str) -> bool {
        let key = Self::key(thread, turn);
        let mut finished = self.0.lock().expect("never poisoned");
        if !finished.0.insert(key) {
            return false;
        }
        finished.1.push_back(key);
        if finished.1.len() > FINISHED
            && let Some(oldest) = finished.1.pop_front()
        {
            finished.0.remove(&oldest);
        }
        true
    }
}
