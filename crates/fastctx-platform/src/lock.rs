//! Exclusive locks between processes, held on a lock file.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// An exclusive lock, held until dropped. The lock file stays in place;
/// only the lock on it is released.
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

impl FileLock {
    /// Waits until the lock is free, then takes it.
    pub fn acquire(path: &Path) -> io::Result<Self> {
        let file = open(path)?;
        file.lock()?;
        Ok(Self { _file: file })
    }

    /// Takes the lock if it is free; `None` if another holder has it.
    pub fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        let file = open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }
}

fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
}
