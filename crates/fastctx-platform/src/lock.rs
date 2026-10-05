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
        #[cfg(not(target_os = "android"))]
        file.lock()?;
        #[cfg(target_os = "android")]
        android_lock(&file, false)?;
        Ok(Self { _file: file })
    }

    /// Takes the lock if it is free; `None` if another holder has it.
    pub fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        let file = open(path)?;
        #[cfg(not(target_os = "android"))]
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
        #[cfg(target_os = "android")]
        match android_lock(&file, true) {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_os = "android")]
fn android_lock(file: &File, nonblocking: bool) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // Android's libc implements flock, while Rust 1.92's File locking API
    // returns Unsupported here. Closing the owned descriptor releases it.
    let operation = libc::LOCK_EX | if nonblocking { libc::LOCK_NB } else { 0 };
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
}
