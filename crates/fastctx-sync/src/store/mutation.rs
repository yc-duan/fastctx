//! Registered local proposals and authority-owned accepted versions are distinct.

use super::Error;
use crate::{
    cursor::{LocalSequence, Revision},
    identity::{ObjectId, OperationId, SourceId},
    registry::{Asset, Kind, REGISTRY},
};
pub use fastctx_types::sync::mutation::*;

/// Constructed by the common gate, not by a feature-specific storage backend.
pub struct AtomicMutation {
    request: Submission,
    kinds: Vec<Kind<Asset>>,
}
impl AtomicMutation {
    pub fn new(request: Submission) -> Result<Self, Error> {
        request.validate_bounds().map_err(Error::Invalid)?;
        let kinds = request
            .objects
            .iter()
            .map(|object| {
                let kind = REGISTRY.asset(&object.kind).map_err(Error::Invalid)?;
                if matches!(
                    kind.registration().conflict,
                    crate::registry::ConflictRule::LocalPending
                        | crate::registry::ConflictRule::MigrationSource
                        | crate::registry::ConflictRule::SingleWriter
                ) {
                    return Err(Error::Invalid(
                        "streams and local carriers cannot be edited as authority-owned records".into(),
                    ));
                }
                Ok(kind)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { request, kinds })
    }
    pub fn request(&self) -> &Submission {
        &self.request
    }
    pub fn kinds(&self) -> &[Kind<Asset>] {
        &self.kinds
    }
}

#[derive(Clone, Debug)]
pub enum Expected {
    Absent,
    Revision(Revision),
}
#[derive(Clone, Debug)]
pub enum Origin {
    Local,
    Replicated { source: crate::identity::Producer },
    Imported { source: SourceId },
}
#[derive(Clone, Debug)]
pub struct RuntimeChange {
    pub operation: OperationId,
    pub key: String,
    pub expected: Expected,
    pub body: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ProjectionChange {
    pub operation: OperationId,
    pub object: ObjectId,
    pub source: LocalSequence,
    pub body: Vec<u8>,
}
