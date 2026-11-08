//! Cryptographic adapters preserve original evidence and validate its authority.

use crate::{catalog::SourceBatch, store::Error};
pub use fastctx_types::sync::proof::*;

/// Implemented by an identity-bound adapter with an already loaded private key.
/// Typed statements select closed domains; signing performs no filesystem I/O.
pub trait ProofSigner: Send + Sync {
    fn accepted(&self, value: &CommittedFragment) -> Result<SignedAcceptanceFragment, Error>;
    fn source(&self, value: &SourceBatch) -> Result<SignedSourceBatch, Error>;
    fn epoch(&self, value: &EpochDescriptor) -> Result<ConfirmedEpoch, Error>;
    fn release(&self, value: &ProducerRelease) -> Result<SignedStatement, Error>;
    fn exposure(
        &self,
        value: &crate::exposure::ExposureReceipt,
    ) -> Result<crate::exposure::SignedExposureReceipt, Error>;
    fn rotation(&self, value: &HubKeyRotation) -> Result<SignedStatement, Error>;
}

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
