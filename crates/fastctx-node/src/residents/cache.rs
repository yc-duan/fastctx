//! Inactive resident projections live on disk. The canonical conversation
//! records rebuild this per-node cache at startup; it is never a second
//! durable authority.

use super::{Live, here};
use fastctx_types::ResidentId;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
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
    /// Those with a copy on disk, written when they last ended.
    on_disk: HashSet<ResidentId>,
    /// Put in, or handed out to be changed, since the last release.
    dirty: HashSet<ResidentId>,
    /// Those held only as an unchanged copy of the one on disk: the ones let
    /// go first when too many are held.
    cold: HashSet<ResidentId>,
    /// A resident was put in, taken out or handed out to be changed since
    /// this was last asked (`take_touched`).
    touched: bool,
}

/// How many residents put aside are kept read in memory, so the node's
/// rounds over everyone read them from disk once rather than every time.
const KEPT: usize = 4096;

/// A resident no longer at work: not here, or here with nothing open and
/// no turn under way.
fn ended(live: &Live) -> bool {
    !here(live)
        || (live.connections == 0
            && !live.host_present
            && live.now.as_ref().is_none_or(|now| now.activity != fastctx_types::residents::Activity::Busy))
}

/// Writes a cached projection in place. The cache is rebuilt from the
/// conversation records at every start and touched only under the state
/// lock, so neither a flush to disk nor a staged copy would buy anything.
fn put(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

impl Cache {
    /// The residents held at work, not those kept read after they ended.
    #[cfg(test)]
    pub fn cached(&self) -> usize {
        self.entries.len() - self.cold.len()
    }

    pub fn new(root: PathBuf) -> io::Result<Self> {
        fastctx_platform::ensure_private_dir(&root)?;
        let mut on_disk = HashSet::new();
        for entry in std::fs::read_dir(&root)? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "json")
                && let Some(id) = path.file_stem().and_then(|id| id.to_str()).and_then(|id| id.parse().ok())
            {
                on_disk.insert(id);
            }
        }
        Ok(Self { root, entries: HashMap::new(), on_disk, dirty: HashSet::new(), cold: HashSet::new(), touched: false })
    }

    fn path(&self, id: &ResidentId) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    pub fn contains_key(&self, id: &ResidentId) -> bool {
        self.entries.contains_key(id) || self.on_disk.contains(id)
    }

    pub fn insert(&mut self, id: ResidentId, live: Live) {
        self.touched = true;
        self.cold.remove(&id);
        self.dirty.insert(id.clone());
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
        self.cold.remove(id);
        self.dirty.insert(id.clone());
        self.entries.get_mut(id)
    }

    /// Whether `id` is on record, read into memory when it was put aside.
    fn load(&mut self, id: &ResidentId) -> bool {
        if self.entries.contains_key(id) {
            return true;
        }
        if !self.on_disk.contains(id) {
            return false;
        }
        let bytes = match std::fs::read(self.path(id)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.on_disk.remove(id);
                return false;
            }
            Err(error) => failed(&format!("reading resident cache for {id}"), error),
        };
        let live = required(serde_json::from_slice(&bytes), "decoding resident cache");
        self.entries.insert(id.clone(), live);
        self.cold.insert(id.clone());
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
        self.dirty.remove(id);
        self.cold.remove(id);
        if self.on_disk.remove(id) {
            match std::fs::remove_file(self.path(id)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => failed(&format!("removing resident cache for {id}"), error),
            }
        }
    }

    pub fn ids(&self) -> Box<dyn Iterator<Item = ResidentId> + Send> {
        let ids: Vec<_> =
            self.on_disk.iter().chain(self.entries.keys().filter(|id| !self.on_disk.contains(*id))).cloned().collect();
        Box::new(ids.into_iter())
    }

    /// Writes down those changed since the last release that are no longer
    /// at work, and keeps them read; those still at work stay in memory
    /// alone. Past `KEPT`, the oldest kept copies are let go.
    pub fn release(&mut self) -> io::Result<()> {
        for id in std::mem::take(&mut self.dirty) {
            let Some(live) = self.entries.get(&id) else { continue };
            if !ended(live) {
                continue;
            }
            let bytes = serde_json::to_vec(live).map_err(io::Error::other)?;
            put(&self.path(&id), &bytes)?;
            self.on_disk.insert(id.clone());
            self.cold.insert(id);
        }
        if self.cold.len() > KEPT {
            let gone: Vec<_> = self.cold.iter().take(self.cold.len() - KEPT).cloned().collect();
            for id in gone {
                self.cold.remove(&id);
                self.entries.remove(&id);
            }
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
            put(&self.path(&key), &serde_json::to_vec(&value).expect("index value serializes")),
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
