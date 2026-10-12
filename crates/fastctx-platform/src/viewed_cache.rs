//! Coordination of a viewing cache shared by a node and interface processes.

use std::io;
use std::path::{Path, PathBuf};

use crate::{FileLock, Permissions, ensure_private_dir, publish};

/// The lock and generation stay in place when the contents are cleared.
/// A writer captures the generation before doing work and checks it again
/// under this lock before publishing; work from before a clear is discarded.
pub struct Turn {
    root: PathBuf,
    _lock: FileLock,
    generation: u64,
}

impl Turn {
    pub fn acquire(root: &Path) -> io::Result<Self> {
        ensure_private_dir(root)?;
        let lock = FileLock::acquire(&root.join(".lock"))?;
        let generation = match std::fs::read_to_string(root.join(".generation")) {
            Ok(text) => text.parse().map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        Ok(Self { root: root.to_path_buf(), _lock: lock, generation })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Invalidates outstanding writes, before deleting any contents.
    pub fn invalidate(&mut self) -> io::Result<()> {
        let next =
            self.generation.checked_add(1).ok_or_else(|| io::Error::other("viewed cache generation overflow"))?;
        publish(&self.root.join(".generation"), next.to_string().as_bytes(), Permissions::Private)?;
        self.generation = next;
        Ok(())
    }

    /// Clears the cache contents, retaining its coordination files.
    pub fn clear(&mut self) -> io::Result<()> {
        self.invalidate()?;
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if [".lock", ".generation", ".requests"].iter().any(|name| entry.file_name() == *name) {
                continue;
            }
            crate::owned_directory::delete(&entry.path())?;
        }
        Ok(())
    }
}

/// The node's cache deletion engine uses the same operation as its viewers.
/// Do not remove the root itself while another process may be using it.
pub fn clear(root: &Path) -> io::Result<()> {
    Turn::acquire(root)?.clear()
}
