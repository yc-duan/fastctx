//! Cryptographic adapters preserve original evidence and validate its authority.

use crate::{catalog::SourceBatch, store::Error};
pub use fastctx_types::sync::proof::*;

/// Implemented by fastctx-net using its closed signing domains and pinned keys.
/// A valid signature alone does not authorize a producer for a stream or scope.
pub trait ProofVerifier: Send + Sync {
    fn accepted(
        &self,
        signed: &SignedAcceptanceFragment,
        authority: &crate::catalog::Authority,
    ) -> Result<CommittedFragment, Error>;
    fn source(&self, signed: &SignedSourceBatch, epoch: &EpochDescriptor) -> Result<SourceBatch, Error>;
    fn epoch(&self, signed: &ConfirmedEpoch, identity: &HubSigningIdentity) -> Result<EpochDescriptor, Error>;
    fn rotation(&self, signed: &SignedStatement, previous: &HubSigningIdentity) -> Result<HubKeyRotation, Error>;
}
