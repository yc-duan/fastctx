//! Real keys, signed and checked as devices and hubs sign and check: a
//! measure of a frame's way includes its signature.

use fastctx_content::v2::{Authorizer, EpochFact, EpochKind, Header, HubSigner, HubVerifier, Signed, Signer, Verifier};
use fastctx_net::sign::{Domain, PublicKey, SecretKey, Signature};
use std::sync::Arc;

/// A device's key, ordering a stream.
pub(crate) struct Device(pub(crate) SecretKey);

impl Signer for Device {
    fn device(&self) -> [u8; 32] {
        *self.0.public().as_bytes()
    }

    fn key(&self) -> [u8; 32] {
        *self.0.public().as_bytes()
    }

    fn sign(&self, header: &[u8]) -> Vec<u8> {
        self.0.sign(Domain::SyncSourceBatch, header).to_bytes().to_vec()
    }
}

/// Checks a frame against the key its header names, as a hub and a replica
/// check it.
pub(crate) struct Checked;

impl Verifier for Checked {
    fn verify(&self, header: &Header, header_bytes: &[u8], signature: &[u8]) -> bool {
        let Ok(signature) = <[u8; 64]>::try_from(signature) else { return false };
        PublicKey::from_bytes(header.key).verify(Domain::SyncSourceBatch, header_bytes, &Signature::from_bytes(signature)).is_ok()
    }
}

/// A hub's service key, signing what it says of frames.
pub(crate) struct Hub(pub(crate) SecretKey);

impl HubSigner for Hub {
    fn sign(&self, statement: &[u8]) -> Vec<u8> {
        self.0.sign(Domain::AssetHub, statement).to_bytes().to_vec()
    }
}

/// Takes the hub's word as given: an epoch's descriptor is accepted once,
/// outside what is measured.
pub(crate) struct Trusted;

impl HubVerifier for Trusted {
    fn verify(&self, _: &[u8], _: &[u8]) -> bool {
        true
    }
}

/// The first epoch of `stream` of `scope`, for the device `device`.
pub(crate) fn first_epoch(scope: &str, stream: &str, device: [u8; 32]) -> Signed {
    let fact = EpochFact {
        scope: scope.into(),
        stream: stream.into(),
        epoch: 0,
        device,
        key: device,
        start: 0,
        kind: EpochKind::Source,
        predecessor: None,
        follows: [0; 32],
    };
    Signed { fact: fact.encode(), signature: Vec::new() }
}

/// The kinds a hub judges records by: a timeline's here.
pub(crate) fn kinds() -> Arc<dyn Authorizer> {
    Arc::new(fastctx_content::v2::Kinds::new().register::<fastctx_content::v2::Timeline>())
}
