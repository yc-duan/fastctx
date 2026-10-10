//! Separate app-server working directories, with durable ownership of short names.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use sha2::{Digest as _, Sha256};

/// Reuses an owned directory across path aliases and node restarts. Legacy
/// short names are retained; an actual collision gets another short name,
/// rather than sharing sockets, thread caches or model selections.
pub(super) fn get(root: &Path, home: &Path) -> io::Result<PathBuf> {
    ensure_private_dir(root)?;
    for entry in std::fs::read_dir(root)? {
        let directory = entry?.path();
        if let Some(owner) = owner(&directory)?
            && crate::homes::same_dir(&owner, home)
        {
            return Ok(directory);
        }
    }
    let identity = serde_json::to_vec(home)?;
    let mut number = 0u64;
    loop {
        let key = if number == 0 {
            crate::homes::key(home)
        } else {
            let mut digest = Sha256::new();
            digest.update(&identity);
            digest.update(number.to_le_bytes());
            digest.finalize()[..4].iter().map(|byte| format!("{byte:02x}")).collect()
        };
        let directory = root.join(key);
        ensure_private_dir(&directory)?;
        match publish_new(&directory.join("home.json"), &identity, Permissions::Private) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if owner(&directory)?.is_some_and(|owner| crate::homes::same_dir(&owner, home)) {
                    return Ok(directory);
                }
            }
            Err(error) => return Err(error),
        }
        number = number.checked_add(1).ok_or_else(|| io::Error::other("Codex home directory names exhausted"))?;
    }
}

fn owner(directory: &Path) -> io::Result<Option<PathBuf>> {
    match std::fs::read(directory.join("home.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => Ok(None),
        Err(error) => Err(error),
    }
}
