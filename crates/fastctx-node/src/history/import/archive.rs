//! A durable list of verified originals makes archival resumable across upgrades.

use crate::record_migration::archive;
use fastctx_platform::disk;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Original {
    source: PathBuf,
    target: PathBuf,
    signature: archive::Signature,
}

impl Original {
    pub(crate) fn of(world: &Path, owner: &str, source: &Path) -> io::Result<Self> {
        let signature = archive::fingerprint(source)?;
        Ok(Self { source: source.into(), target: archive::verified_path(world, owner, source, &signature)?, signature })
    }

    pub(crate) fn source(&self) -> &Path {
        &self.source
    }

    pub(crate) fn target(&self) -> &Path {
        &self.target
    }

    pub(crate) fn verify_archived(&self) -> io::Result<()> {
        if archive::fingerprint(&self.target)? != self.signature {
            return Err(io::Error::other("the archived import source changed after verification"));
        }
        Ok(())
    }

    pub(crate) fn readable(&self) -> &Path {
        if disk::is_there(&self.source) { &self.source } else { &self.target }
    }

    pub(crate) fn verify(&self) -> io::Result<()> {
        if archive::fingerprint(self.readable())? != self.signature {
            return Err(io::Error::other("the standalone history source changed after import began"));
        }
        Ok(())
    }

    pub(crate) fn finish(&self) -> io::Result<()> {
        if let Ok(metadata) = disk::metadata(&self.source)
            && metadata.permissions().readonly()
        {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            std::fs::set_permissions(&self.source, permissions)?;
        }
        archive::verified_at(&self.source, &self.target, &self.signature)
    }
}

pub(crate) fn collect(world: &Path, owner: &str, path: &Path, originals: &mut Vec<Original>) -> io::Result<()> {
    let metadata = match disk::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() {
        for entry in disk::read_dir(path)? {
            collect(world, owner, &entry?.path(), originals)?;
        }
    } else if metadata.is_file() {
        originals.push(Original::of(world, owner, path)?);
    } else {
        return Err(io::Error::other("a migration source is not an ordinary file or directory"));
    }
    Ok(())
}

pub(crate) fn empty(path: &Path) -> io::Result<()> {
    let entries = match disk::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            empty(&entry.path())?;
        }
    }
    match disk::remove_dir(path) {
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        result => result,
    }
}
