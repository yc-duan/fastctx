use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::transaction::Snapshot;

use super::Error;

/// Refuse every redirected path component, including Windows junctions.
pub(super) fn check_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "legacy path must be absolute without '..'"));
    }
    let mut part = PathBuf::new();
    for component in path.components() {
        part.push(component);
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&part) {
            Ok(metadata) if crate::disk::is_link(&metadata) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "legacy path contains a link or reparse point",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(super) fn read(path: &Path) -> Result<Snapshot, Error> {
    check_path(path).map_err(super::io)?;
    Ok(Snapshot::read(path)?)
}

pub(super) fn entries(path: &Path) -> io::Result<Vec<PathBuf>> {
    check_path(path)?;
    let directory = match fs::read_dir(path) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut entries = directory.map(|entry| entry.map(|entry| entry.path())).collect::<io::Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn plain(snapshot: &Snapshot) -> bool {
    snapshot.state.as_ref().is_some_and(|state| !state.attributes.has_named_data() && state.attributes.writable())
}

pub(super) fn released(version: &str) -> bool {
    matches!(version, "0.1.0" | "0.1.1" | "0.2.0" | "0.2.1" | "0.2.2" | "0.2.3" | "0.2.4" | "0.2.5" | "0.2.6")
}
