//! Every installed kind supplies explicit ingestion and projection behavior.

use crate::{catalog::ObjectHead, manifest::BlobId, store::Error};

/// Registration names a required handler. A service cannot advertise a kind
/// until that handler is installed; opaque legacy input remains quarantined.
/// Bodies are bounded record or manifest pages, never an entire asset file.
pub trait KindHandler: Send + Sync {
    fn kind(&self) -> &'static str;
    fn validate(&self, schema: u16, body: &[u8]) -> Result<(), Error>;
    fn references(&self, schema: u16, body: &[u8]) -> Result<Vec<BlobId>, Error>;
    fn import_legacy(&self, shape: &str, body: &[u8]) -> Result<Vec<u8>, Error>;
    fn project_legacy(&self, head: &ObjectHead, body: &[u8], generation: u16) -> Result<Vec<u8>, Error>;
    fn project_local(&self, head: &ObjectHead, body: &[u8]) -> Result<Vec<u8>, Error>;
}
