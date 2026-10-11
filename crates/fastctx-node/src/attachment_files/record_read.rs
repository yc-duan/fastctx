//! Verify an observed record prefix while allowing its host to append.

use fastctx_platform::file_read::{Snapshot, open, snapshot};
use sha2::{Digest as _, Sha256};
use std::io::{self, Read, Seek as _};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct Prefix<R> {
    source: R,
    hash: Sha256,
}

impl<R> Prefix<R> {
    pub(super) fn new(source: R) -> Self {
        Self { source, hash: Sha256::new() }
    }
    pub(super) fn digest(self) -> [u8; 32] {
        self.hash.finalize().into()
    }
}

impl<R: Read> Read for Prefix<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let read = self.source.read(bytes)?;
        self.hash.update(&bytes[..read]);
        Ok(read)
    }
}

pub(super) fn check(
    path: &Path,
    mut file: std::fs::File,
    before: &Snapshot,
    observed: [u8; 32],
    cancel: &AtomicBool,
) -> io::Result<()> {
    let changed = || io::Error::new(io::ErrorKind::Interrupted, "the host record changed while reading its picture");
    let after = snapshot(&file, &file.metadata()?)?;
    let current = open(path)?;
    let current = snapshot(&current, &current.metadata()?)?;
    if *before == after && *before == current {
        return Ok(());
    }
    if !before.same_file(&after)
        || !before.same_file(&current)
        || after.size < before.size
        || current.size < before.size
        || (after.size == before.size && current.size == before.size)
    {
        return Err(changed());
    }
    // Appending is normal during a turn. Re-read exactly the observed
    // prefix to distinguish it from a rewrite accompanied by growth.
    file.rewind()?;
    let mut reading = Prefix::new(file.take(before.size));
    let mut block = [0; 8192];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "image read was cancelled"));
        }
        if reading.read(&mut block)? == 0 {
            break;
        }
    }
    if reading.digest() != observed {
        return Err(changed());
    }
    let current = open(path)?;
    let current = snapshot(&current, &current.metadata()?)?;
    if !before.same_file(&current) || current.size < before.size {
        return Err(changed());
    }
    Ok(())
}
