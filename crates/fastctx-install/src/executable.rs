//! The stable host launch path and retained Windows executable images.

use std::fs;
use std::path::{Path, PathBuf};

use crate::disk::{self, Identity};
use crate::transaction::{FileError, Snapshot};

/// No environment lookup: tests and callers explicitly choose the user's home.
pub fn installed_path(home: &Path) -> PathBuf {
    home.join(".fastctx/bin").join(if cfg!(windows) { "fastctx.exe" } else { "fastctx" })
}

const PREFIX: &str = ".fastctx-exe-retired-";

pub(crate) fn retain(snapshot: &Snapshot) -> Result<Option<PathBuf>, FileError> {
    if !cfg!(windows) {
        return Ok(None);
    }
    snapshot.verify()?;
    let Some(state) = &snapshot.state else { return Ok(None) };
    let prefix = format!("{PREFIX}{:016x}-{:032x}-", state.identity.volume, state.identity.file);
    let path = disk::unique_path(snapshot.path.parent().expect("absolute path"), &prefix)
        .map_err(|e| FileError::io(&snapshot.path, "name retained image", e))?;
    fs::hard_link(&snapshot.path, &path).map_err(|e| FileError::io(&snapshot.path, "retain image", e))?;
    // The caller owns cleanup from this point, including a subsequent failed
    // comparison. The identity embedded in the name makes later cleanup narrow.
    Ok(Some(path))
}

/// Retries deletion of recognized, identity-matching image links in this one
/// installation's bin directory. Does not recurse or follow links. Unrecognized
/// files are left alone; every recognized file that could not be deleted is reported.
pub fn cleanup_retired(installed: &Path) -> Vec<FileError> {
    let Some(parent) = installed.parent() else { return Vec::new() };
    match fs::symlink_metadata(parent) {
        Ok(metadata) if disk::is_link(&metadata) || !metadata.is_dir() => return vec![FileError::changed(parent)],
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => return vec![FileError::io(parent, "inspect image directory", error)],
    }
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => return vec![FileError::io(parent, "list retained images", error)],
    };
    let mut errors = Vec::new();
    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) if retained_identity(&entry.path()).is_some() => paths.push(entry.path()),
            Ok(_) => {}
            Err(error) => errors.push(FileError::io(parent, "list retained images", error)),
        }
    }
    paths.sort();
    for path in paths {
        if let Err(error) = delete_retired(&path) {
            errors.push(error);
        }
    }
    errors
}

pub(crate) fn delete_retired(path: &Path) -> Result<(), FileError> {
    let expected = retained_identity(path).ok_or_else(|| FileError::changed(path))?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(FileError::io(path, "inspect retained image", error)),
    };
    disk::ordinary(&metadata).map_err(|e| FileError::io(path, "inspect retained image", e))?;
    let file = disk::open(path).map_err(|e| FileError::io(path, "open retained image", e))?;
    if disk::identity(&file).map_err(|e| FileError::io(path, "identify retained image", e))? != expected {
        return Err(FileError::changed(path));
    }
    disk::delete(&file, path).map_err(|e| FileError::io(path, "delete retained image", e))
}

fn retained_identity(path: &Path) -> Option<Identity> {
    let name = path.file_name()?.to_str()?.strip_prefix(PREFIX)?;
    let mut parts = name.split('-');
    let volume = parts.next()?;
    let file = parts.next()?;
    let random = parts.next()?;
    if parts.next().is_some()
        || volume.len() != 16
        || file.len() != 32
        || random.len() != 32
        || !random.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(Identity { volume: u64::from_str_radix(volume, 16).ok()?, file: u128::from_str_radix(file, 16).ok()? })
}
