//! In-memory ownership of job files, separate from each job's record updates.
//! Record leases exclude overlapping collection. Creation anchors keep the
//! jobs and output directories in place while older children can be collected.
//! Registration and collection are atomic decisions that never perform IO.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_platform::doing::{MemoryMutex, Rank, SyncMutex};

pub(crate) type Gate = crate::leases::Lease<PathBuf, SyncMutex<()>>;

pub(crate) struct Files {
    state: Arc<MemoryMutex<State>>,
    gates: crate::leases::Registry<PathBuf, SyncMutex<()>>,
    pub(crate) due: Arc<Mutex<HashSet<PathBuf>>>,
    pub(crate) changed: Arc<tokio::sync::Notify>,
}

#[derive(Default)]
struct State {
    readers: BTreeMap<PathBuf, usize>,
    directories: BTreeMap<PathBuf, usize>,
    collecting: Vec<PathBuf>,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            state: Arc::new(MemoryMutex::shared(Rank::JOB_FILES, "job files", State::default())),
            gates: Default::default(),
            due: Arc::default(),
            changed: Arc::default(),
        }
    }
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

impl Files {
    /// Reserves a record or a complete subtree before any IO.
    pub(crate) fn hold(&self, path: &Path) -> Option<Lease> {
        let mut state = self.state.lock();
        if state.collecting.iter().any(|other| overlaps(path, other)) {
            return None;
        }
        *state.readers.entry(path.to_path_buf()).or_default() += 1;
        Some(Lease {
            state: self.state.clone(),
            due: self.due.clone(),
            changed: self.changed.clone(),
            path: Some(path.to_path_buf()),
            directory: false,
        })
    }

    /// Keeps a creator's directory in place without reserving its existing
    /// children: an older job in the same directory can still be collected.
    pub(crate) fn hold_directory(&self, path: &Path) -> Option<Lease> {
        let mut state = self.state.lock();
        if state.collecting.iter().any(|other| path.starts_with(other)) {
            return None;
        }
        *state.directories.entry(path.to_path_buf()).or_default() += 1;
        Some(Lease {
            state: self.state.clone(),
            due: self.due.clone(),
            changed: self.changed.clone(),
            path: Some(path.to_path_buf()),
            directory: true,
        })
    }

    /// Serializes one record's updates. Reserve its files before taking it.
    /// Several gates are taken in the lexical order of their displayed paths.
    pub(crate) fn gate(&self, path: &Path) -> Gate {
        self.gates.get_or_insert_with(&path.to_path_buf(), || {
            SyncMutex::new(Rank::DISK, "job record", path.to_string_lossy().into_owned(), ())
        })
    }

    pub(crate) fn reading(&self, paths: &[PathBuf]) -> bool {
        let state = self.state.lock();
        state.readers.keys().any(|reader| paths.iter().any(|path| overlaps(reader, path)))
            || state.directories.keys().any(|directory| paths.iter().any(|path| directory.starts_with(path)))
    }

    pub(crate) fn collect(&self, paths: Vec<PathBuf>) -> Result<Collection, Held> {
        let mut state = self.state.lock();
        if state.readers.keys().any(|reader| paths.iter().any(|path| overlaps(reader, path)))
            || state.directories.keys().any(|directory| paths.iter().any(|path| directory.starts_with(path)))
        {
            return Err(Held::Read);
        }
        mark(&mut state, &paths)?;
        Ok(Collection { state: self.state.clone(), paths })
    }
}

pub(crate) struct Lease {
    state: Arc<MemoryMutex<State>>,
    due: Arc<Mutex<HashSet<PathBuf>>>,
    changed: Arc<tokio::sync::Notify>,
    path: Option<PathBuf>,
    directory: bool,
}

impl Lease {
    /// Exchanges this last reader for a collection claim without an unowned gap.
    pub(crate) fn collect(mut self) -> Result<Collection, Self> {
        if self.directory {
            return Err(self);
        }
        let mut state = self.state.lock();
        let path = self.path.as_ref().expect("active lease");
        if state.readers.iter().any(|(other, count)| overlaps(path, other) && (other != path || *count > 1))
            || state.directories.keys().any(|directory| directory.starts_with(path))
            || state.collecting.iter().any(|other| overlaps(path, other))
        {
            drop(state);
            return Err(self);
        }
        let path = self.path.take().expect("active lease");
        state.readers.remove(&path);
        state.collecting.push(path.clone());
        Ok(Collection { state: self.state.clone(), paths: vec![path] })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let Some(path) = &self.path else { return };
        let last = {
            let mut state = self.state.lock();
            let holders = if self.directory { &mut state.directories } else { &mut state.readers };
            let count = holders.get_mut(path).expect("registered lease");
            *count -= 1;
            if *count == 0 {
                holders.remove(path);
                true
            } else {
                false
            }
        };
        if last {
            let due = self.due.lock().expect("never poisoned");
            let waiting = if self.directory { due.iter().any(|due| overlaps(due, path)) } else { due.contains(path) };
            if waiting {
                self.changed.notify_one();
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Held {
    Read,
    Collecting,
}

fn mark(state: &mut State, paths: &[PathBuf]) -> Result<(), Held> {
    if state.collecting.iter().any(|other| paths.iter().any(|path| overlaps(path, other))) {
        return Err(Held::Collecting);
    }
    state.collecting.extend_from_slice(paths);
    Ok(())
}

pub(crate) struct Collection {
    state: Arc<MemoryMutex<State>>,
    paths: Vec<PathBuf>,
}

impl Drop for Collection {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        for path in &self.paths {
            let at = state.collecting.iter().position(|other| other == path).expect("registered collection");
            state.collecting.swap_remove(at);
        }
    }
}
