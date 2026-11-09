//! Confirmation belongs to a process, not to the mere existence of a directory.
use std::{
    collections::HashMap,
    fs, io,
    path::{Component, Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
};

#[derive(Default)]
struct Cache {
    entries: Mutex<HashMap<PathBuf, Arc<Entry>>>,
}
#[derive(Default)]
struct Entry {
    phase: Mutex<Phase>,
    changed: Condvar,
}
#[derive(Default)]
enum Phase {
    #[default]
    Unconfirmed,
    Confirming,
    Prepared,
}
struct Confirmation {
    entry: Arc<Entry>,
    prepared: bool,
}
impl Entry {
    fn begin(self: Arc<Self>) -> Option<Confirmation> {
        let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match *phase {
                Phase::Prepared => return None,
                Phase::Unconfirmed => {
                    *phase = Phase::Confirming;
                    drop(phase);
                    return Some(Confirmation { entry: self, prepared: false });
                }
                Phase::Confirming => phase = self.changed.wait(phase).unwrap_or_else(|e| e.into_inner()),
            }
        }
    }
}
impl Confirmation {
    fn complete(mut self) {
        self.prepared = true;
    }
}
impl Drop for Confirmation {
    fn drop(&mut self) {
        *self.entry.phase.lock().unwrap_or_else(|e| e.into_inner()) =
            if self.prepared { Phase::Prepared } else { Phase::Unconfirmed };
        self.entry.changed.notify_all();
    }
}
impl Cache {
    fn prepare(&self, path: &Path, before_confirm: &impl Fn(&Path)) -> io::Result<()> {
        let entry =
            self.entries.lock().unwrap_or_else(|e| e.into_inner()).entry(path.to_path_buf()).or_default().clone();
        let Some(confirmation) = entry.begin() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            self.prepare(parent, before_confirm)?;
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) => plain(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => super::create_directory(path)?,
            Err(error) => return Err(error),
        }
        before_confirm(path);
        if path.parent().is_some() {
            super::sync_parent(path)?;
        }
        confirmation.complete();
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
