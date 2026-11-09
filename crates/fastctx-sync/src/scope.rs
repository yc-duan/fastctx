//! A scope is installed from original hub evidence, never from a caller's label.

use crate::{
    catalog::Lifecycle,
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
        let old = previous.descriptor();
        let next = self.descriptor();
        if next == old {
            return Ok(());
        }
        if next.scope != old.scope
            || !next.owner.same_identity(&old.owner)
            || next.visibility != old.visibility
            || next.revision == old.revision
            || next.acl_revision <= old.acl_revision
            || old.lifecycle == Lifecycle::Deleted
            || !same_subject(&next.subject, &old.subject)
        {
            return Err(Error::Denied("scope update changes identity or fails to advance its ACL fence".into()));
        }
        Ok(())
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
    if descriptor.format != 1
        || &descriptor.scope != expected
        || descriptor.scope.hub != identity.hub
        || descriptor.acl_revision.0 == 0
        || descriptor.revision.revision.0 == 0
        || descriptor.lifecycle == Lifecycle::Removed
    {
        return Err(Error::Invalid("invalid scope identity or revision".into()));
    }
    if matches!(descriptor.subject, ScopeSubject::Administrators) != matches!(descriptor.owner, ScopeOwner::Hub) {
        return Err(Error::Invalid("only administrator scopes are hub-owned".into()));
    }
    if let ScopeVisibility::SinceGrant { opened_acl } = descriptor.visibility
        && (!matches!(descriptor.subject, ScopeSubject::Machine { .. })
            || opened_acl.0 == 0
            || opened_acl > descriptor.acl_revision)
    {
        return Err(Error::Invalid("grant visibility requires a valid immutable machine segment fence".into()));
    }
    if let ScopeSubject::Conversation { shared_on, born_seal, .. } = &descriptor.subject
        && (shared_on.is_some() != born_seal.is_some() || born_seal.as_ref().is_some_and(|seal| seal.len() > 4096))
    {
        return Err(Error::Invalid("shared conversation origin needs its bounded machine seal".into()));
    }
    Ok(VerifiedScope {
        descriptor,
        source: StreamCursor { scope: batch.scope, stream: batch.stream, epoch: batch.epoch, committed: batch.through },
        proof,
    })
}

fn same_subject(one: &ScopeSubject, other: &ScopeSubject) -> bool {
    match (one, other) {
        (ScopeSubject::Account, ScopeSubject::Account)
        | (ScopeSubject::Administrators, ScopeSubject::Administrators) => true,
        (ScopeSubject::Conversation { conversation: a, .. }, ScopeSubject::Conversation { conversation: b, .. }) => {
            a == b
        }
        (ScopeSubject::Project { project: a, .. }, ScopeSubject::Project { project: b, .. }) => a == b,
        (ScopeSubject::Machine { device: a }, ScopeSubject::Machine { device: b })
        | (ScopeSubject::Device { device: a }, ScopeSubject::Device { device: b }) => a == b,
        _ => false,
    }
}
