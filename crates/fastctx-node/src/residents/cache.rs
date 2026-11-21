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
    /// The residents put in, taken out or handed out to be changed since
    /// this was last asked (`take_touched`).
    touched: HashSet<ResidentId>,
    /// Where the count of changes stood as each resident was last put in,
    /// taken out or handed out to be changed (`stamp`), and the residents
    /// by that count, so that those changed since a count are found
    /// without going through all.
    stamps: HashMap<ResidentId, u64>,
    stamped: std::collections::BTreeMap<u64, Vec<ResidentId>>,
    /// The residents gone, in the order they went, with the count each
    /// went at: the latest `GONE_KEPT` are told of as changed; one who asks
    /// from before the last let go (`forgotten`) is told all may have.
    gone: std::collections::VecDeque<(u64, ResidentId)>,
    forgotten: u64,
    putting: Arc<Aside>,
}

/// How many residents gone are still told of as changed (`Cache::stamp`).
const GONE_KEPT: usize = 1024;

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
            touched: HashSet::new(),
            stamps: HashMap::new(),
            stamped: std::collections::BTreeMap::new(),
            gone: std::collections::VecDeque::new(),
            forgotten: 0,
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
        self.touched.insert(id.clone());
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
        self.touched.insert(id.clone());
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

    /// The residents that may have changed since this was last asked.
    pub fn take_touched(&mut self) -> HashSet<ResidentId> {
        std::mem::take(&mut self.touched)
    }

    /// `residents` may have changed as the count of changes came to `at`.
    pub fn stamp(&mut self, residents: HashSet<ResidentId>, at: u64) {
        for id in residents {
            if let Some(before) = self.stamps.insert(id.clone(), at)
                && let Some(alike) = self.stamped.get_mut(&before)
            {
                alike.retain(|other| *other != id);
                if alike.is_empty() {
                    self.stamped.remove(&before);
                }
            }
            if !self.contains_key(&id) {
                self.gone.push_back((at, id.clone()));
            }
            self.stamped.entry(at).or_default().push(id);
        }
        // A resident gone is told of to those who ask in time; its stamp
        // is not kept for ever.
        while self.gone.len() > GONE_KEPT {
            let Some((went, id)) = self.gone.pop_front() else { break };
            // Here again since, under a later stamp: that one stands.
            if self.stamps.get(&id) != Some(&went) || self.contains_key(&id) {
                continue;
            }
            self.stamps.remove(&id);
            if let Some(alike) = self.stamped.get_mut(&went) {
                alike.retain(|other| *other != id);
                if alike.is_empty() {
                    self.stamped.remove(&went);
                }
            }
            self.forgotten = self.forgotten.max(went);
        }
    }

    /// The residents that may have changed since the count of changes
    /// stood at `seen`: one gone since is among them. None where one gone
    /// since is no longer kept: all may have.
    pub fn stamped_since(&self, seen: u64) -> Option<Vec<ResidentId>> {
        if seen < self.forgotten {
            return None;
        }
        Some(self.stamped.range(seen.saturating_add(1)..).flat_map(|(_, alike)| alike.iter().cloned()).collect())
    }

    /// Forgets `id` altogether, here and on disk.
    pub fn remove(&mut self, id: &ResidentId) {
        self.touched.insert(id.clone());
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
