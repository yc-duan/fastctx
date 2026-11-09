//! Opaque generation names never accept a path from a peer or a manifest entry.
use crate::{
    identity::{Digest, OperationId, ScopeKey},
    manifest::{Encoding, Representation},
    store::Error,
};
use fastctx_platform::{disk, durable_file};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct FileKey {
    scope: ScopeKey,
    content: Digest,
    representation: Digest,
    encoding: Encoding,
    generation: OperationId,
}
impl FileKey {
    pub(super) fn new(value: &Representation, generation: OperationId) -> Self {
        Self {
            scope: value.content.id.domain.clone(),
            content: value.content.id.digest.clone(),
            representation: value.id(),
            encoding: value.envelope.encoding,
            generation,
        }
    }
    pub(super) fn json(&self) -> Result<String, Error> {
        serde_json::to_string(self).map_err(|error| Error::Invalid(error.to_string()))
    }
    pub(super) fn decode(text: &str, value: &Representation) -> Result<Self, Error> {
        let key: Self = serde_json::from_str(text).map_err(|error| Error::Corrupt(error.to_string()))?;
        if key.scope != value.content.id.domain
            || key.content != value.content.id.digest
            || key.representation != value.id()
            || key.encoding != value.envelope.encoding
        {
            return Err(Error::Corrupt("representation storage key differs from its verified identity".into()));
        }
        Ok(key)
    }
    /// Only an unpublished slot may change encoded bytes. Its unique physical
    /// generation stays the same, so an abandoned partial file is not orphaned.
    pub(super) fn rebind(&self, value: &Representation) -> Result<Self, Error> {
        if self.scope != value.content.id.domain
            || self.content != value.content.id.digest
            || self.encoding != value.envelope.encoding
        {
            return Err(Error::Invalid("an encoding slot cannot change content or encoding".into()));
        }
        Ok(Self { representation: value.id(), ..self.clone() })
    }
    fn path(&self, root: &Path) -> PathBuf {
        root.join(self.scope.hub.to_string()).join(self.scope.scope.as_str()).join(&self.content.as_str()[..2]).join(
            format!(
                "{}.{}.{}",
                self.content.as_str(),
                match self.encoding {
                    Encoding::Raw => "raw",
                    Encoding::Zstd => "zstd",
                },
                self.generation.as_str()
            ),
        )
    }
}

pub(super) struct Files {
    root: PathBuf,
    _lock: fastctx_platform::FileLock,
}
impl Files {
    pub(super) fn open(root: PathBuf) -> Result<Self, Error> {
        durable_file::prepare_directory(&root).map_err(io_error)?;
        let root = disk::canonicalize(root).map_err(io_error)?;
        let lock =
            fastctx_platform::FileLock::try_acquire(&root.join(".lock")).map_err(io_error)?.ok_or(Error::Busy)?;
        Ok(Self { root, _lock: lock })
    }
    pub(super) fn put(&self, key: &FileKey, bytes: &[u8]) -> Result<(), Error> {
        durable_file::write_unpublished(&key.path(&self.root), bytes).map_err(io_error)
    }
    pub(super) fn read(&self, key: &FileKey, bytes: u32) -> Result<Vec<u8>, Error> {
        let body = durable_file::read_bounded(&key.path(&self.root), bytes).map_err(io_error)?;
        if body.len() != bytes as usize {
            return Err(Error::Corrupt("published block has an invalid length".into()));
        }
        Ok(body)
    }

    pub(super) fn delete(&self, key: &FileKey) -> Result<(), Error> {
        durable_file::delete_unreferenced(&key.path(&self.root)).map_err(io_error)
    }
}
fn io_error(error: io::Error) -> Error {
    match error.kind() {
        io::ErrorKind::StorageFull => Error::Full,
        io::ErrorKind::NotFound => Error::Corrupt("published content file is missing".into()),
        io::ErrorKind::InvalidData => Error::Corrupt(error.to_string()),
        _ => Error::Unavailable(format!("content filesystem operation failed: {error}")),
    }
}
