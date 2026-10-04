//! Filesystem mechanisms used only by the installer.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(unix)]
#[path = "disk/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "disk/windows.rs"]
mod imp;
pub(crate) use imp::*;

/// Establishes each parent name durably before creating its descendants.
pub(crate) fn ensure_private_dir(path: &Path) -> io::Result<()> {
    let path = std::path::absolute(path)?;
    let mut missing = Vec::new();
    let mut ancestor = path.as_path();
    while !ancestor.try_exists()? {
        missing.push(ancestor.to_owned());
        ancestor = ancestor.parent().ok_or_else(|| io::Error::other("directory has no existing ancestor"))?;
    }
    for directory in missing.iter().rev() {
        fs::create_dir(directory)?;
        sync_parent(directory)?;
    }
    fastctx_platform::ensure_private_dir(&path)?;
    sync_directory(&path)?;
    // This also finishes a creation interrupted before its parent flush.
    sync_parent(&path)
}

pub(crate) fn publish_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fastctx_platform::publish(path, bytes, fastctx_platform::Permissions::Private)?;
    fs::OpenOptions::new().read(true).write(true).open(path)?.sync_all()?;
    sync_parent(path)
}

/// A filesystem object, independent of the name used to reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub(crate) volume: u64,
    pub(crate) file: u128,
}

pub(crate) fn unique_path(parent: &Path, prefix: &str) -> io::Result<PathBuf> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
    Ok(parent.join(format!("{prefix}{suffix}")))
}

pub(crate) fn ordinary(metadata: &fs::Metadata) -> io::Result<()> {
    if !metadata.is_file() || is_link(metadata) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "expected a regular file, not a link or special file"));
    }
    Ok(())
}

pub(crate) struct Staged {
    pub path: PathBuf,
    pub file: File,
    pub present: bool,
}

impl Staged {
    pub fn create(target: &Path, mode: u32) -> io::Result<Self> {
        let path = unique_path(target.parent().expect("absolute file path"), ".fastctx-stage-")?;
        let file = create(&path, mode)?;
        Ok(Self { path, file, present: true })
    }

    pub fn discard(&mut self) -> io::Result<()> {
        if self.present {
            delete_staged(&self.file, &self.path)?;
            self.present = false;
        }
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        // Explicit cleanup reports errors; this also covers unwinding before
        // a transaction starts publishing.
        let _ = self.discard();
    }
}
