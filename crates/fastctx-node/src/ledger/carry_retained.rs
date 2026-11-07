//! Rejected history signatures stay beside the ledger without becoming ledger entries.

use crate::record_migration::archive::{Signature, fingerprint};
use fastctx_platform::disk;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Retained {
    source: PathBuf,
    target: PathBuf,
    signature: Signature,
}

impl Retained {
    pub(super) fn new(source: &Path, directory: &Path) -> io::Result<Self> {
        Ok(Self {
            source: source.into(),
            target: directory.join(source.file_name().expect("signature filename")),
            signature: fingerprint(source)?,
        })
    }

    pub(crate) fn source(&self) -> &Path {
        &self.source
    }

    pub(crate) fn readable(&self) -> io::Result<&Path> {
        let path = match (disk::exists(&self.source)?, disk::exists(&self.target)?) {
            (true, false) => &self.source,
            (false, true) => &self.target,
            (true, true) => return Err(io::Error::other("a retained signature exists in both ledger locations")),
            (false, false) => {
                return Err(io::Error::other("a retained signature is missing from both ledger locations"));
            }
        };
        if fingerprint(path)? != self.signature {
            return Err(io::Error::other("a retained signature changed during import"));
        }
        Ok(path)
    }

    pub(super) fn finish(&self) -> io::Result<()> {
        if self.readable()? == self.target {
            return Ok(());
        }
        fastctx_platform::ensure_private_dir(self.target.parent().expect("retained signatures directory"))?;
        disk::rename(&self.source, &self.target)
    }
}
