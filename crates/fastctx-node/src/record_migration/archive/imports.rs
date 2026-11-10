//! Import publication and expiry share a gate so an old archive can be reused.

use fastctx_platform::disk;
use serde::Serialize;
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

static GENERATION: Mutex<u64> = Mutex::new(0);

pub(crate) fn with_import_gate<T>(work: impl FnOnce() -> T) -> T {
    let _gate = GENERATION.lock().expect("never poisoned");
    work()
}

pub(crate) fn keep_import(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut generation = GENERATION.lock().expect("never poisoned");
    // A publication can fail after replacing the file, so every attempt
    // invalidates a collector's snapshot, including failed publications.
    *generation = generation.wrapping_add(1);
    fastctx_net::link::state::write(path, value)
}

pub(crate) fn import_journals(parent: &Path, name: &str) -> io::Result<Vec<PathBuf>> {
    let entries = match disk::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let path = entry.path().join(name);
            if disk::exists(&path)? {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

pub(super) struct References {
    world: PathBuf,
    generation: u64,
    targets: HashSet<PathBuf>,
}

impl References {
    pub(super) fn new(world: &Path) -> io::Result<Self> {
        loop {
            let generation = *GENERATION.lock().expect("never poisoned");
            let targets = targets(world)?;
            if generation == *GENERATION.lock().expect("never poisoned") {
                return Ok(Self { world: world.into(), generation, targets });
            }
        }
    }

    pub(super) fn delete_if_unreferenced(&mut self, path: &Path) -> io::Result<bool> {
        loop {
            let generation = GENERATION.lock().expect("never poisoned");
            if self.generation == *generation {
                if self.targets.contains(path) {
                    return Ok(false);
                }
                disk::remove_file(path)?;
                return Ok(true);
            }
            drop(generation);
            *self = Self::new(&self.world)?;
        }
    }
}

fn targets(world: &Path) -> io::Result<HashSet<PathBuf>> {
    let mut targets: HashSet<_> = crate::ledger::carry::archive_targets(world)?.into_iter().collect();
    targets.extend(crate::history::import::state::archive_targets(world)?);
    Ok(targets)
}
