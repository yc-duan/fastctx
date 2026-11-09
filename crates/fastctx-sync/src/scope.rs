//! A scope is installed from original hub evidence, never from a caller's label.

use crate::{
    cursor::StreamCursor,
    identity::{Digest, Producer, ScopeKey},
    proof::{HubSigningIdentity, ProofVerifier},
    store::Error,
};
pub use fastctx_types::sync::scope::*;

/// Verified identity evidence is not a current permission grant. The writer
/// installs it only in authenticated feed/snapshot order, then applies its ACL
/// and publication fences before exposing any associated object.
#[derive(Clone, Debug)]
pub struct VerifiedScope {
    descriptor: ScopeDescriptor,
    source: StreamCursor,
    proof: ScopeProof,
}
impl VerifiedScope {
    pub fn descriptor(&self) -> &ScopeDescriptor {
        &self.descriptor
    }
    pub fn source(&self) -> &StreamCursor {
        &self.source
    }
    pub fn proof(&self) -> &ScopeProof {
        &self.proof
    }

    /// This does not order authority epochs. The caller has already established
    /// feed order; every changed description must advance its ACL generation.
    pub fn validate_successor(&self, previous: &Self) -> Result<(), Error> {
        self.descriptor.validate_successor(&previous.descriptor).map_err(Error::Denied)
    }
}

pub fn verify(
    expected: &ScopeKey,
    proof: ScopeProof,
    identity: &HubSigningIdentity,
    verifier: &dyn ProofVerifier,
) -> Result<VerifiedScope, Error> {
    let budget = fastctx_types::sync::PAGE_BYTES as usize;
    if proof.payload.0.len() > budget
        || proof.epoch.proof.data.0.len() > budget
        || proof.batch.proof.data.0.len() > budget
    {
        return Err(Error::Invalid("scope evidence exceeds its metadata budget".into()));
    }
    if serde_json::to_vec(&proof).map_err(|error| Error::Invalid(error.to_string()))?.len() > budget {
        return Err(Error::Invalid("encoded scope evidence exceeds one page".into()));
    }
    let epoch = verifier.epoch(&proof.epoch, identity)?;
    if epoch.producer != Producer::Hub || epoch.kind != "account.directory" || epoch.schema != 1 {
        return Err(Error::Denied("scope identities require the registered hub directory source".into()));
    }
    let batch = verifier.source(&proof.batch, &epoch)?;
    if batch.records.len() != 1 {
        return Err(Error::Invalid("a scope proof must disclose exactly one directory record".into()));
    }
    let payload = &batch.records[0].payload;
    if payload.decoded_bytes as usize != proof.payload.0.len() || payload.id.digest != Digest::of(&proof.payload.0) {
        return Err(Error::Invalid("scope payload differs from its signed content binding".into()));
    }
    let descriptor: ScopeDescriptor =
        serde_json::from_slice(&proof.payload.0).map_err(|error| Error::Invalid(error.to_string()))?;
    if &descriptor.scope != expected || descriptor.scope.hub != identity.hub {
        return Err(Error::Invalid("invalid scope identity or revision".into()));
    }
    descriptor.validate().map_err(Error::Invalid)?;
    Ok(VerifiedScope {
        descriptor,
        source: StreamCursor { scope: batch.scope, stream: batch.stream, epoch: batch.epoch, committed: batch.through },
        proof,
    })
}
