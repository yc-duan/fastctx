//! Membership tombstones survive host reconnects. Archive records without
//! changing the paths held by running job supervisors and output writers.

use std::io;
use std::sync::Arc;

use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use jiff::Timestamp;

use super::Residents;

pub fn removal(world: &WorldDir, id: &ResidentId) -> io::Result<Option<Timestamp>> {
    match std::fs::read(world.removed(id)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn refused(id: &ResidentId) -> Failure {
    Failure::new(format!(
        "{id} was removed from the World; this session can no longer use World tools or be woken; open a new session to become a resident"
    ))
}

impl Residents {
    pub fn active(&self, id: &ResidentId) -> Result<(), Failure> {
        let state = self.lock();
        let live = state.live.get(id).ok_or_else(|| Failure::new(format!("no resident {id} lives here")))?;
        if live.removed.is_some() { Err(refused(id)) } else { Ok(()) }
    }

    pub fn removed(&self, id: &ResidentId) -> Option<Timestamp> {
        self.lock().live.get(id).and_then(|live| live.removed)
    }

    pub fn management(&self, id: &ResidentId) -> Result<Arc<tokio::sync::RwLock<()>>, Failure> {
        self.lock()
            .live
            .get(id)
            .map(|live| live.management.clone())
            .ok_or_else(|| Failure::new(format!("no resident {id} lives here")))
    }

    /// Mark membership removed first. If archiving is interrupted, startup
    /// finishes it; a reconnect can never turn partial removal into arrival.
    pub fn remove(&self, world: &WorldDir, id: &ResidentId) -> io::Result<Vec<String>> {
        let mut state = self.lock();
        let _transaction = crate::resident_storage::lock();
        {
            let live = state.live.get_mut(id).ok_or_else(|| io::Error::other("unknown resident"))?;
            if live.removed.is_none() {
                let at = Timestamp::now();
                let path = world.removed(id);
                ensure_private_dir(path.parent().expect("tombstone parent"))?;
                publish_new(&path, &serde_json::to_vec(&at).map_err(io::Error::other)?, Permissions::Private)?;
                live.removed = Some(at);
            }
        }
        archive(world, id)
    }
}

fn archive(world: &WorldDir, id: &ResidentId) -> io::Result<Vec<String>> {
    let mut paths = Vec::new();
    for parent in world.resident_parents()? {
        let base = parent.join(id.as_str());
        if !base.is_dir() {
            continue;
        }
        let destination = base.join("archive");
        ensure_private_dir(&destination)?;
        for entry in std::fs::read_dir(&base)? {
            let entry = entry?;
            if matches!(entry.file_name().to_str(), Some("archive" | "jobs" | "out")) {
                continue;
            }
            let target = destination.join(entry.file_name());
            if target.exists() {
                return Err(io::Error::other(format!(
                    "both {} and {} exist; neither was overwritten",
                    entry.path().display(),
                    target.display()
                )));
            }
            std::fs::rename(entry.path(), target)?;
        }
        paths.push(crate::world::native(&destination));
    }
    Ok(paths)
}

pub(super) fn recover(world: &WorldDir) -> io::Result<()> {
    let directory = world.path().join("removed-residents");
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        let Some(id) = path.file_stem().and_then(|name| name.to_str()).and_then(|name| name.parse().ok()) else {
            continue;
        };
        removal(world, &id)?;
        archive(world, &id)?;
    }
    Ok(())
}
