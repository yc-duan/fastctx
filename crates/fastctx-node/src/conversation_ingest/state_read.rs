//! Carries the source reader's revision fence through byte-oriented decoder IO.
//! Supersession is recovery from the current checkpoint, never a host rewrite.
use fastctx_sync::{
    manifest::{BlobId, ManifestReply, ManifestRoot},
    store::{
        Error,
        ingest::{SourceIngest, SourceTarget, StateRead, StateReply},
    },
};
use fastctx_types::sync::cursor::Revision;
use std::{fmt, io};

/// One immutable checkpoint revision for an entire decoder step. New uploads
/// remain the caller's admitted in-memory bytes until the real writer adopts
/// them; this reader never falls back to ordinary object or representation IO.
pub struct SourceState<'a> {
    ingest: &'a dyn SourceIngest,
    target: SourceTarget,
    checkpoint: Revision,
}
impl<'a> SourceState<'a> {
    pub fn new(ingest: &'a dyn SourceIngest, target: SourceTarget, checkpoint: Revision) -> Self {
        Self { ingest, target, checkpoint }
    }
    pub async fn manifest(&self, root: ManifestRoot) -> io::Result<ManifestReply> {
        let reply = self
            .ingest
            .read_state(self.target.clone(), StateRead::Manifest { checkpoint: self.checkpoint, root: root.clone() })
            .await
            .map_err(read_error)?;
        manifest_reply(reply, &root)
    }
    pub async fn block(&self, id: BlobId) -> io::Result<Vec<u8>> {
        let reply = self
            .ingest
            .read_state(self.target.clone(), StateRead::Block { checkpoint: self.checkpoint, id })
            .await
            .map_err(read_error)?;
        block_reply(reply)
    }
}
pub fn manifest_reply(reply: StateReply, expected: &ManifestRoot) -> io::Result<ManifestReply> {
    match reply {
        StateReply::Manifest(page) if &page.root == expected => Ok(page),
        StateReply::Superseded { current } => Err(Superseded { current }.into_io()),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "source state returned a different manifest")),
    }
}
pub fn block_reply(reply: StateReply) -> io::Result<Vec<u8>> {
    match reply {
        StateReply::Bytes(bytes) if bytes.len() <= fastctx_types::sync::BLOCK_BYTES as usize => Ok(bytes),
        StateReply::Superseded { current } => Err(Superseded { current }.into_io()),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "source state returned an invalid decoded block")),
    }
}
fn read_error(error: Error) -> io::Error {
    let kind = match error {
        Error::Busy => io::ErrorKind::WouldBlock,
        Error::Denied(_) | Error::Revoked => io::ErrorKind::PermissionDenied,
        Error::Corrupt(_) => io::ErrorKind::InvalidData,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Superseded {
    pub current: Option<Revision>,
}
impl fmt::Display for Superseded {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.current {
            Some(revision) => write!(out, "source checkpoint superseded by revision {}", revision.0),
            None => out.write_str("source checkpoint was discarded"),
        }
    }
}
impl std::error::Error for Superseded {}
impl Superseded {
    pub fn into_io(self) -> io::Error {
        io::Error::new(io::ErrorKind::Interrupted, self)
    }
    pub fn from_io(error: &io::Error) -> Option<&Self> {
        error.get_ref()?.downcast_ref()
    }
}
