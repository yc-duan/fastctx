//! Bounded reads from an explicitly claimed host source. A path locates bytes;
//! it never supplies ownership, canonical conversation identity or producer epoch.
use super::{
    checkpoint::Anchor,
    jsonl::{SealReason, StableSeal},
    lexical::WINDOW_BYTES,
};
use fastctx_platform::file_read::{self, Snapshot};
use fastctx_types::{
    file_identity::FileIdentity,
    sync::{cursor::ByteCount, identity::Digest},
};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// Check the opened handle, since checking the pathname before open races a
/// replacement and the platform opener intentionally permits directories.
pub(super) fn ordinary_file(path: &Path) -> io::Result<File> {
    let file = file_read::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Claude source is not an ordinary file"));
    }
    Ok(file)
}

pub struct SourceFile {
    file: File,
    path: PathBuf,
    identity: FileIdentity,
    read_bytes: u64,
}
pub struct Window {
    pub offset: ByteCount,
    pub bytes: Vec<u8>,
    pub before: Snapshot,
    pub after: Snapshot,
}
impl SourceFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = ordinary_file(path)?;
        let snapshot = file_read::snapshot(&file, &file.metadata()?)?;
        Ok(Self { file, path: path.to_owned(), identity: snapshot.identity(), read_bytes: 0 })
    }
    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }
    pub fn read_bytes(&self) -> u64 {
        self.read_bytes
    }
    pub fn snapshot(&self) -> io::Result<Snapshot> {
        file_read::snapshot(&self.file, &self.file.metadata()?)
    }
    pub fn read_window(&mut self, offset: ByteCount, limit: usize) -> io::Result<Window> {
        if limit == 0 || limit > WINDOW_BYTES {
            return Err(io::Error::other("source read exceeds one window"));
        }
        let before = self.snapshot()?;
        if offset.0 > before.size {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "source was truncated before its cursor"));
        }
        self.file.seek(SeekFrom::Start(offset.0))?;
        let mut bytes = vec![0; limit.min(usize::try_from(before.size - offset.0).unwrap_or(usize::MAX))];
        let mut used = 0;
        while used < bytes.len() {
            let size = self.file.read(&mut bytes[used..])?;
            if size == 0 {
                break;
            }
            used += size;
            self.read_bytes = self
                .read_bytes
                .checked_add(size as u64)
                .ok_or_else(|| io::Error::other("source read counter overflow"))?;
        }
        bytes.truncate(used);
        let after = self.snapshot()?;
        let path = ordinary_file(&self.path)?;
        let named = file_read::snapshot(&path, &path.metadata()?)?;
        if before.identity() != self.identity
            || after.identity() != self.identity
            || named.identity() != self.identity
            || after.size < before.size
            || before.size == after.size && before.change_stamp()? != after.change_stamp()?
        {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "source changed while reading a window"));
        }
        Ok(Window { offset, bytes, before, after })
    }
    pub fn check_anchor(&mut self, anchor: &Anchor) -> io::Result<bool> {
        if anchor.bytes == 0 || anchor.bytes as usize > WINDOW_BYTES {
            return Err(io::Error::other("anchor exceeds one window"));
        }
        let window = self.read_window(anchor.offset, anchor.bytes as usize)?;
        Ok(window.bytes.len() == anchor.bytes as usize && Digest::of(&window.bytes) == anchor.digest)
    }
    /// Capture once the host/world has explicitly sealed this opened source.
    /// `confirm_seal` performs the second observation after the final pending read.
    pub fn seal_sample(&self) -> io::Result<Snapshot> {
        self.snapshot()
    }
    pub fn confirm_seal(&self, reason: SealReason, before: &Snapshot, end: ByteCount) -> io::Result<StableSeal> {
        let after = self.snapshot()?;
        let seal = StableSeal {
            reason,
            identity: after.identity(),
            before: before.change_stamp()?,
            after: after.change_stamp()?,
        };
        if before.identity() != self.identity {
            return Err(io::Error::other("seal sample belongs to another opened file"));
        }
        seal.validate(&self.identity, end.0)?;
        Ok(seal)
    }
}
