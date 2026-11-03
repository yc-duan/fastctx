//! A program put in its place without holding it in memory. Programs run to
//! hundreds of megabytes, and a file transaction (`transaction.rs`) keeps
//! each file it changes whole in memory, the one it replaces too, and reads
//! that one whole again to verify it: a renewal held about four times a
//! program at once, more than a small server had. Here the new program is
//! copied beside its place through a fixed buffer, read back against what
//! was read, synced and renamed over its place in one step. The program it
//! replaces is linked aside first, as a Windows image still mapped must be
//! (`executable::retain_archived`): a failure after the rename puts it back,
//! and the link goes with the retired images' cleanup.

use crate::disk::{self, Attributes, Identity, Staged};
use crate::executable;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::SystemTime;

const BUFFER: usize = 1 << 20;

/// A file as it was looked at: changed after, the publication stops.
#[derive(PartialEq, Eq)]
struct Seen {
    identity: Identity,
    modified: SystemTime,
    len: u64,
}

fn seen(file: &File) -> io::Result<Seen> {
    let metadata = file.metadata()?;
    disk::ordinary(&metadata)?;
    Ok(Seen { identity: disk::identity(file)?, modified: metadata.modified()?, len: metadata.len() })
}

fn changed(path: &Path) -> io::Error {
    io::Error::other(format!("{} changed while a program was put in place", path.display()))
}

/// Puts the program at `source` at `target`, replacing what is there.
pub(crate) fn publish(source: &Path, target: &Path) -> io::Result<()> {
    let parent = target.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no directory"))?;
    let directory = disk::identity(&disk::open_directory(parent)?)?;
    let mut from = disk::open(source)?;
    let original = seen(&from)?;
    let mut old = match disk::open(target) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let was = old.as_ref().map(seen).transpose()?;
    let attributes = old.as_ref().map(|file| Attributes::read(file, target)).transpose()?;
    let mut staged = Staged::create(target, if old.is_some() { 0o600 } else { 0o755 })?;
    let result = (|| {
        if let Some(attributes) = &attributes {
            // The staging name is protected before it holds the program.
            attributes.protect(&staged.file)?;
        }
        let digest = copy(&mut from, &mut staged.file)?;
        if seen(&from)? != original || seen(&disk::open(source)?)? != original {
            return Err(changed(source));
        }
        if let Some(attributes) = &attributes {
            attributes.apply(&staged.file, &staged.path)?;
        }
        staged.file.sync_all()?;
        if hash(&mut disk::open(&staged.path)?)? != digest {
            return Err(io::Error::other("the program copied differs from what was read"));
        }
        if disk::identity(&disk::open_directory(parent)?)? != directory {
            return Err(changed(parent));
        }
        let replace = old.is_some();
        let kept = match old.take() {
            Some(file) => {
                if seen(&disk::open(target)?)? != *was.as_ref().expect("seen with its file") {
                    return Err(changed(target));
                }
                // Let go before the rename: only the link aside holds it.
                let kept = executable::retain_archived(parent, target, &file)?;
                drop(file);
                Some(kept)
            }
            None => None,
        };
        disk::publish(&staged.file, &staged.path, target, replace)?;
        staged.present = disk::stage_survives_publication(replace);
        if let Err(error) = disk::sync_parent(target) {
            // Put back what was there, as a transaction would roll it back.
            if let Some(kept) = &kept {
                let _ = disk::publish(&disk::open(kept)?, kept, target, true);
            }
            return Err(error);
        }
        Ok(())
    })();
    let discarded = staged.discard();
    result?;
    discarded
}

/// Copies `from` into `to` through a fixed buffer, giving its digest.
fn copy(from: &mut File, to: &mut impl Write) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; BUFFER];
    loop {
        let read = match from.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        digest.update(&buffer[..read]);
        to.write_all(&buffer[..read])?;
    }
    Ok(digest.finalize().into())
}

fn hash(file: &mut File) -> io::Result<[u8; 32]> {
    copy(file, &mut io::sink())
}
