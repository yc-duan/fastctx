//! Inactive resident projections live on disk. The canonical conversation
//! records rebuild this per-node cache at startup; it is never a second
//! durable authority.

use super::{Live, here};
use fastctx_types::ResidentId;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

pub(super) fn failed(operation: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("fastctx node: {operation}: {error}; runtime state cannot be trusted, stopping the node");
    crate::ending::exit(1)
}

fn required<T>(result: Result<T, impl std::fmt::Display>, operation: &str) -> T {
    result.unwrap_or_else(|error| failed(operation, error))
}

pub(super) struct Cache {
    root: PathBuf,
    entries: HashMap<ResidentId, Live>,
    clean: HashMap<ResidentId, Vec<u8>>,
    /// A resident was put in, taken out or handed out to be changed since
    /// this was last asked (`take_touched`).
    touched: bool,
}

impl Cache {
    #[cfg(test)]
    pub fn cached(&self) -> usize {
        self.entries.len()
    }
    pub fn new(root: PathBuf) -> io::Result<Self> {
        fastctx_platform::ensure_private_dir(&root)?;
        Ok(Self { root, entries: HashMap::new(), clean: HashMap::new(), touched: false })
    }

    fn path(&self, id: &ResidentId) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    pub fn contains_key(&self, id: &ResidentId) -> bool {
        self.entries.contains_key(id) || required(self.path(id).try_exists(), "reading resident cache")
    }

    pub fn insert(&mut self, id: ResidentId, live: Live) {
        self.touched = true;
        self.entries.insert(id, live);
    }

    pub fn get(&mut self, id: &ResidentId) -> Option<&Live> {
        self.load(id).then(|| &self.entries[id])
    }

    pub fn get_mut(&mut self, id: &ResidentId) -> Option<&mut Live> {
        if !self.load(id) {
            return None;
        }
        self.touched = true;
        self.entries.get_mut(id)
    }

    /// Whether `id` is on record, read into memory when it was put aside.
    fn load(&mut self, id: &ResidentId) -> bool {
        if !self.entries.contains_key(id) {
            let bytes = match std::fs::read(self.path(id)) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return false,
                Err(error) => failed(&format!("reading resident cache for {id}"), error),
            };
            let live = required(serde_json::from_slice(&bytes), "decoding resident cache");
            self.entries.insert(id.clone(), live);
            self.clean.insert(id.clone(), bytes);
        }
        true
    }

    /// Whether a resident may have changed since this was last asked.
    pub fn take_touched(&mut self) -> bool {
        std::mem::take(&mut self.touched)
    }

    /// Forgets `id` altogether, here and on disk.
    pub fn remove(&mut self, id: &ResidentId) {
        self.touched = true;
        self.entries.remove(id);
        self.clean.remove(id);
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => failed(&format!("removing resident cache for {id}"), error),
        }
    }

    pub fn ids(&self) -> Box<dyn Iterator<Item = ResidentId> + Send> {
        let active: Vec<_> = self
            .entries
            .keys()
            .filter(|id| !required(self.path(id).try_exists(), "reading resident cache"))
            .cloned()
            .collect();
        let uncached: std::collections::HashSet<_> = active.iter().cloned().collect();
        let saved = required(std::fs::read_dir(&self.root), "listing resident cache").filter_map(move |entry| {
            let path = required(entry, "reading resident cache entry").path();
            if path.extension().is_none_or(|extension| extension != "json") {
                return None;
            }
            let id = path.file_stem().and_then(|id| id.to_str()).and_then(|id| id.parse().ok())?;
            (!uncached.contains(&id)).then_some(id)
        });
        Box::new(saved.chain(active))
    }

    pub fn release(&mut self) -> io::Result<()> {
        let ended: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, live)| {
                !here(live)
                    || (live.connections == 0
                        && !live.host_present
                        && live.now.as_ref().is_none_or(|now| now.activity != fastctx_types::residents::Activity::Busy))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ended {
            let bytes = serde_json::to_vec(&self.entries[&id]).map_err(io::Error::other)?;
            if self.clean.get(&id) != Some(&bytes) {
                fastctx_platform::publish(&self.path(&id), &bytes, fastctx_platform::Permissions::Private)?;
            }
            self.entries.remove(&id);
            self.clean.remove(&id);
        }
        if self.entries.is_empty() {
            self.entries = HashMap::new();
            self.clean = HashMap::new();
        } else {
            self.entries.shrink_to_fit();
            self.clean.shrink_to_fit();
        }
        Ok(())
    }
}

pub(crate) struct Index {
    root: PathBuf,
}

impl Index {
    pub fn new(root: PathBuf) -> io::Result<Self> {
        fastctx_platform::ensure_private_dir(&root)?;
        Ok(Self { root })
    }

    fn path(&self, key: &(impl Serialize + ?Sized)) -> PathBuf {
        let bytes = serde_json::to_vec(key).expect("index key serializes");
        self.root.join(format!("{:x}.json", Sha256::digest(bytes)))
    }

    pub fn insert(&self, key: impl Serialize, value: impl Serialize) {
        required(
            fastctx_platform::publish(
                &self.path(&key),
                &serde_json::to_vec(&value).expect("index value serializes"),
                fastctx_platform::Permissions::Private,
            ),
            "writing resident index",
        );
    }

    pub fn get<V: DeserializeOwned>(&self, key: &(impl Serialize + ?Sized)) -> Option<V> {
        match std::fs::read(self.path(key)) {
            Ok(bytes) => Some(required(serde_json::from_slice(&bytes), "decoding resident index")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => failed("reading resident index", error),
        }
    }

    pub fn contains(&self, key: &(impl Serialize + ?Sized)) -> bool {
        required(self.path(key).try_exists(), "reading resident index")
    }

    pub fn remove(&self, key: &impl Serialize) {
        match std::fs::remove_file(self.path(key)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => failed("removing resident index", error),
        }
    }
}

pub(super) fn prepare(world: &Path) -> io::Result<PathBuf> {
    let root = world.join("resident-cache");
    match std::fs::remove_dir_all(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fastctx_platform::ensure_private_dir(&root)?;
    Ok(root)
}
