//! Identity-bound signing and verification of transferable synchronization facts.
mod signer;
mod validation;
mod verifier;
pub use signer::IdentitySigner;
pub use verifier::IdentityVerifier;

use crate::sign::{Domain, PublicKey, Signature};
use fastctx_sync::{
    proof::{ProofPurpose, SignedStatement},
    store::Error,
};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, Write};

// Proofs expand as base64 inside metadata frames; reserve space for their wrapper.
const MAX_PROOF_BYTES: usize = fastctx_types::sync::PAGE_BYTES as usize * 3 / 4 - 4096;

fn domain(purpose: ProofPurpose) -> Domain {
    match purpose {
        ProofPurpose::SourceBatchV1 => Domain::SyncSourceBatch,
        ProofPurpose::HubSourceBatchV1 => Domain::SyncHubSourceBatch,
        ProofPurpose::AcceptedOperationV1 => Domain::SyncAcceptedOperation,
        ProofPurpose::ProducerReleaseV1 => Domain::SyncProducerRelease,
        ProofPurpose::EpochHandoverV1 => Domain::SyncEpochHandover,
        ProofPurpose::ExposureReceiptV1 => Domain::SyncExposureReceipt,
        ProofPurpose::HubKeyRotationV1 => Domain::SyncHubKeyRotation,
    }
}
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_PROOF_BYTES - self.0.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "signed metadata exceeds its frame budget"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, Error> {
    let mut output = Bounded(Vec::new());
    serde_json::to_writer(&mut output, value).map_err(|error| Error::Invalid(error.to_string()))?;
    Ok(output.0)
}
fn open<T: DeserializeOwned>(proof: &SignedStatement, purpose: ProofPurpose, key: PublicKey) -> Result<T, Error> {
    if proof.purpose != purpose || proof.signer.0 != *key.as_bytes() || proof.data.0.len() > MAX_PROOF_BYTES {
        return Err(Error::Denied("signed metadata has a different purpose, signer or budget".into()));
    }
    key.verify(domain(purpose), &proof.data.0, &Signature::from_bytes(proof.signature.0))
        .map_err(|_| Error::Denied("synchronization proof does not verify".into()))?;
    serde_json::from_slice(&proof.data.0).map_err(|error| Error::Invalid(error.to_string()))
}
