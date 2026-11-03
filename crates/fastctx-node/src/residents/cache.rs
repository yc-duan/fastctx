//! Resident projections, held in memory; past a bound, those no longer at
//! work are put aside on disk, never under the residents' lock
//! (`fastctx_platform::aside`). The canonical conversation records rebuild
//! this per-node cache at startup; it is never a second durable authority,
//! and nothing it does ends the node.

use super::{Live, here};
use fastctx_platform::aside::Aside;
use fastctx_types::ResidentId;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) struct Cache {
    root: PathBuf,
    entries: HashMap<ResidentId, Live>,
    /// Those with a copy on disk, or on its way there.
    on_disk: HashSet<ResidentId>,
    /// Put in, or handed out to be changed, since the last release.
    dirty: HashSet<ResidentId>,
    /// No longer at work: the ones put aside first when too many are held.
    cold: HashSet<ResidentId>,
    /// Of those, the ones changed since their copy was last put aside.
    changed: HashSet<ResidentId>,
    /// A resident was put in, taken out or handed out to be changed since
    /// this was last asked (`take_touched`).
    touched: bool,
    putting: Arc<Aside>,
}

/// A resident put aside, to be read back outside the residents' lock
/// (`Cache::aside`, `Cache::install`).
pub(super) struct PutAside {
    path: PathBuf,
    putting: Arc<Aside>,
}

/// How many residents no longer at work are held in memory before the
/// oldest are put aside on disk: all of them on all but the largest
/// machines, so that the node's rounds over everyone never read the disk.
const KEPT: usize = 4096;

/// A resident no longer at work: not here, or here with nothing open and
/// no turn under way.
fn ended(live: &Live) -> bool {
    !here(live)
        || (live.connections == 0
            && !live.host_present
            && live.now.as_ref().is_none_or(|now| now.activity != fastctx_types::residents::Activity::Busy))
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
        for entry in fastctx_platform::disk::read_dir(&root)? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "json")
                && let Some(id) = path.file_stem().and_then(|id| id.to_str()).and_then(|id| id.parse().ok())
            {
                on_disk.insert(id);
            }
        }
        Ok(Self {
            root,
            entries: HashMap::new(),
            on_disk,
            dirty: HashSet::new(),
            cold: HashSet::new(),
            changed: HashSet::new(),
            touched: false,
            putting: Arc::new(Aside::new("resident-cache", "a resident's state")),
        })
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

    /// Where `id` was put aside, when it is not in memory: read back with
    /// `PutAside::read` outside the lock, and installed with `install`.
    pub fn aside(&self, id: &ResidentId) -> Option<PutAside> {
        (!self.entries.contains_key(id) && self.on_disk.contains(id))
            .then(|| PutAside { path: self.path(id), putting: self.putting.clone() })
    }

    /// `id` as it was read back from where it was put aside, unless it came
    /// back meanwhile or went.
    pub fn install(&mut self, id: &ResidentId, read: io::Result<Live>) {
        if self.entries.contains_key(id) || !self.on_disk.contains(id) {
            return;
        }
        match read {
            Ok(live) => {
                self.entries.insert(id.clone(), live);
                self.cold.insert(id.clone());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.on_disk.remove(id);
            }
            // Read again under the lock when asked, and said there.
            Err(_) => {}
        }
    }

    /// Whether `id` is on record, read into memory when it was put aside:
    /// only past `KEPT` does that read the disk, and then under the lock
    /// only where it was not read back first (`aside`). One that cannot be
    /// read back now is said and taken as not here, and read again next time.
    fn load(&mut self, id: &ResidentId) -> bool {
        if self.entries.contains_key(id) {
            return true;
        }
        if !self.on_disk.contains(id) {
            return false;
        }
        let path = self.path(id);
        let bytes = match self.putting.on_its_way(&path).map_or_else(|| fastctx_platform::disk::read(&path), Ok) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.on_disk.remove(id);
                return false;
            }
            Err(error) => {
                eprintln!("fastctx node: reading resident cache for {id}: {error}; it is tried again when asked");
                return false;
            }
        };
        let live = match serde_json::from_slice(&bytes) {
            Ok(live) => live,
            Err(error) => {
                eprintln!("fastctx node: decoding resident cache for {id}: {error}; it is tried again when asked");
                return false;
            }
        };
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
        self.changed.remove(id);
        if self.on_disk.remove(id) {
            self.putting.remove(self.path(id));
        }
    }

    pub fn ids(&self) -> Box<dyn Iterator<Item = ResidentId> + Send> {
        let ids: Vec<_> =
            self.on_disk.iter().chain(self.entries.keys().filter(|id| !self.on_disk.contains(*id))).cloned().collect();
        Box::new(ids.into_iter())
    }

    /// Takes those changed since the last release that are no longer at
    /// work as cold; those still at work stay in memory alone. Past `KEPT`
    /// cold, the oldest are put aside on disk, those changed since they
    /// were last put aside written again, behind this lock and in memory
    /// until written. The disk is never waited for here.
    pub fn release(&mut self) {
        for id in std::mem::take(&mut self.dirty) {
            let Some(live) = self.entries.get(&id) else { continue };
            if !ended(live) {
                continue;
            }
            self.changed.insert(id.clone());
            self.cold.insert(id);
        }
        if self.cold.len() > KEPT {
            let gone: Vec<_> = self.cold.iter().take(self.cold.len() - KEPT).cloned().collect();
            for id in gone {
                if self.changed.contains(&id) || !self.on_disk.contains(&id) {
                    let Some(live) = self.entries.get(&id) else { continue };
                    let Ok(bytes) = serde_json::to_vec(live) else { continue };
                    self.putting.put(self.path(&id), bytes);
                    self.on_disk.insert(id.clone());
                    self.changed.remove(&id);
                }
                self.cold.remove(&id);
                self.entries.remove(&id);
            }
        }
    }
}

impl PutAside {
    /// The resident as it was put aside: what is still on its way, or the
    /// disk.
    pub fn read(&self) -> io::Result<Live> {
        let bytes = match self.putting.on_its_way(&self.path) {
            Some(bytes) => bytes,
            None => fastctx_platform::disk::read(&self.path)?,
        };
        serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

/// A lookup the residents keep for this run, by keys and values as JSON: a
/// session's resident, a resident's inbox. Asked for under the state lock
/// with every host report, it is held in memory, rebuilt from the
/// conversation records at every start as the rest of this cache is.
#[derive(Default)]
pub(crate) struct Index {
    entries: std::sync::Mutex<HashMap<Vec<u8>, Vec<u8>>>,
}

impl Index {
    fn key(key: &(impl Serialize + ?Sized)) -> Vec<u8> {
        serde_json::to_vec(key).expect("index key serializes")
    }

    pub fn insert(&self, key: impl Serialize, value: impl Serialize) {
        let value = serde_json::to_vec(&value).expect("index value serializes");
        self.entries.lock().expect("never poisoned").insert(Self::key(&key), value);
    }

    pub fn get<V: DeserializeOwned>(&self, key: &(impl Serialize + ?Sized)) -> Option<V> {
        let entries = self.entries.lock().expect("never poisoned");
        let bytes = entries.get(&Self::key(key))?;
        Some(serde_json::from_slice(bytes).expect("index values decode as they were encoded"))
    }

    pub fn contains(&self, key: &(impl Serialize + ?Sized)) -> bool {
        self.entries.lock().expect("never poisoned").contains_key(&Self::key(key))
    }

    pub fn remove(&self, key: &impl Serialize) {
        self.entries.lock().expect("never poisoned").remove(&Self::key(key));
    }
}

pub(super) fn prepare(world: &Path) -> io::Result<PathBuf> {
    let root = world.join("resident-cache");
    match fastctx_platform::disk::remove_dir_all(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fastctx_platform::ensure_private_dir(&root)?;
    Ok(root)
}
