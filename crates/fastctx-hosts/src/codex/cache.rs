//! Dormant host projections and terminal identities do not occupy the node heap.

use super::Thread;
use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, hash_map};
use std::io;
use std::path::{Path, PathBuf};

fn failed(operation: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("fastctx node: {operation}: {error}; runtime state cannot be trusted, stopping the node");
    std::process::exit(1)
}

fn required<T>(result: Result<T, impl std::fmt::Display>, operation: &str) -> T {
    result.unwrap_or_else(|error| failed(operation, error))
}

pub(super) fn fresh(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    match std::fs::remove_dir_all(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => failed(&format!("resetting {}", path.display()), error),
    }
    required(ensure_private_dir(&path), "creating host runtime cache");
    path
}

pub(super) fn key(value: &impl serde::Serialize) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).expect("host identity serializes")))
}

pub(super) struct Threads {
    directory: PathBuf,
    generation: u64,
    entries: HashMap<String, Thread>,
    clean: HashMap<String, Vec<u8>>,
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
            clean: HashMap::new(),
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
        let bytes = match std::fs::read(self.path(thread)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => failed("reading Codex thread cache", error),
        };
        let mut stored: Stored = required(serde_json::from_slice(&bytes), "decoding Codex thread cache");
        if stored.generation != self.generation {
            stored.thread.usage = None;
        }
        self.entries.insert(thread.into(), stored.thread);
        self.clean.insert(thread.into(), bytes);
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
        self.clean.remove(thread);
        match std::fs::remove_file(self.path(thread)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => failed("removing Codex thread cache", error),
        }
    }
    pub fn keys(&self) -> hash_map::Keys<'_, String, Thread> {
        self.entries.keys()
    }
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
            if self.clean.get(&id) != Some(&bytes) {
                required(publish(&self.path(&id), &bytes, Permissions::Private), "writing Codex thread cache");
            }
            self.entries.remove(&id);
            self.clean.remove(&id);
        }
        if self.entries.is_empty() {
            self.entries = HashMap::new();
            self.clean = HashMap::new();
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

pub(super) struct Finished(PathBuf);
impl Finished {
    pub fn new(root: &Path) -> Self {
        Self(fresh(root, "runtime-finished"))
    }
    fn path(&self, thread: &str, turn: &str) -> PathBuf {
        self.0.join(key(&(thread, turn)))
    }
    pub fn contains(&self, thread: &str, turn: &str) -> bool {
        required(self.path(thread, turn).try_exists(), "reading Codex turn completion")
    }
    pub fn insert(&self, thread: &str, turn: &str) -> bool {
        match publish_new(&self.path(thread, turn), b"", Permissions::Private) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => failed("recording Codex turn completion", error),
        }
    }
}
