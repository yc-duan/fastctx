//! Short filesystem transactions follow a resident when it changes towns.

use fastctx_types::ResidentId;
use fastctx_types::layout::{ResidentDir, WorldDir};
use fastctx_types::residents::Moved;
use std::io;
use std::sync::{Mutex, MutexGuard};

static TRANSITION: Mutex<()> = Mutex::new(());

pub fn lock() -> MutexGuard<'static, ()> {
    TRANSITION.lock().expect("never poisoned")
}

pub fn current(directory: &ResidentDir) -> io::Result<(MutexGuard<'static, ()>, ResidentDir)> {
    let guard = lock();
    let mut directory = directory.clone();
    let town = directory
        .path()
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| io::Error::other("resident directory has no town"))?;
    let root = if town.file_name().is_some_and(|name| name == "outside") {
        town.parent()
    } else {
        town.parent().and_then(|path| path.parent())
    }
    .ok_or_else(|| io::Error::other("resident directory has no World"))?;
    let world = WorldDir::at(root);
    let id = ResidentId::try_from(
        directory
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io::Error::other("resident directory has no id"))?,
    )
    .map_err(io::Error::other)?;
    let mut seen = std::collections::HashSet::new();
    loop {
        if !seen.insert(directory.path().to_path_buf()) {
            return Err(io::Error::other("resident move records form a cycle"));
        }
        let bytes = match std::fs::read(directory.moved()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((guard, directory)),
            Err(error) => return Err(error),
        };
        let moved: Moved = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        directory = world.resident(moved.to.as_ref(), &id);
    }
}
