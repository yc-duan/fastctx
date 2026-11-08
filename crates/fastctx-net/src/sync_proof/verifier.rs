use super::{open, validation};
use crate::sign::PublicKey;
use fastctx_sync::{
    catalog::{Authority, SourceBatch},
    identity::{DeviceId, HubId},
    metrics::{Latency, Metrics},
    proof::{
        CommittedFragment, ConfirmedEpoch, EpochDescriptor, HubKeyRotation, HubSigningIdentity, ProofPurpose,
        ProofVerifier, SignedAcceptanceFragment, SignedSourceBatch, SignedStatement, SigningKey,
    },
    store::Error,
};
use std::{collections::BTreeSet, sync::Arc, time::Instant};

/// The key set comes from the persisted TLS pin and verified forward rotations,
/// never from a relayed statement's self-declared signer. Old trusted keys remain
/// available to verify facts restored from a replica or backup.
pub struct IdentityVerifier {
    hub: HubId,
    hub_keys: BTreeSet<[u8; 32]>,
    metrics: Arc<dyn Metrics>,
}
impl IdentityVerifier {
    pub fn new(hub: HubId, pinned_keys: impl IntoIterator<Item = SigningKey>, metrics: Arc<dyn Metrics>) -> Self {
        Self { hub, hub_keys: pinned_keys.into_iter().map(|key| key.0).collect(), metrics }
    }
    fn hub_key(&self, hub: &HubId, signer: &SigningKey) -> Result<PublicKey, Error> {
        if hub != &self.hub || !self.hub_keys.contains(&signer.0) {
            return Err(Error::Denied("hub proof is not bound to a pinned signing identity".into()));
        }
        Ok(PublicKey::from_bytes(signer.0))
    }
    fn measured<T>(&self, work: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        let began = Instant::now();
        let result = work();
        self.metrics.elapsed(Latency::Verify, u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX));
        result
    }
}
impl ProofVerifier for IdentityVerifier {
    fn accepted(&self, signed: &SignedAcceptanceFragment, authority: &Authority) -> Result<CommittedFragment, Error> {
        self.measured(|| {
            let key = match authority {
                Authority::Hub => self.hub_key(&self.hub, &signed.proof.signer)?,
                Authority::Device { device, .. } => device_key(device),
            };
            let value: CommittedFragment = open(&signed.proof, ProofPurpose::AcceptedOperationV1, key)?;
            if value.fragment.binding.hub != self.hub || &value.fragment.binding.authority != authority {
                return Err(Error::Denied("accepted proof differs from the confirmed authority".into()));
            }
            if matches!(authority, Authority::Device { .. }) {
                validation::device_kind(&value.fragment.kind)?;
            }
            value.validate().map_err(Error::Invalid)?;
            Ok(value)
        })
    }
    fn source(&self, signed: &SignedSourceBatch, epoch: &EpochDescriptor) -> Result<SourceBatch, Error> {
        self.measured(|| {
            if epoch.scope.hub != self.hub {
                return Err(Error::Denied("source epoch belongs to another hub".into()));
            }
            let value: SourceBatch = open(&signed.proof, ProofPurpose::SourceBatchV1, device_key(&epoch.producer))?;
            if value.scope != epoch.scope
                || value.stream != epoch.stream
                || value.epoch != epoch.epoch
                || value.producer != epoch.producer
                || value.kind != epoch.kind
                || value.schema != epoch.schema
            {
                return Err(Error::Denied("source proof differs from its confirmed producer epoch".into()));
            }
            validation::source(&value)?;
            Ok(value)
        })
    }
    fn epoch(&self, signed: &ConfirmedEpoch, identity: &HubSigningIdentity) -> Result<EpochDescriptor, Error> {
        self.measured(|| {
            self.hub_key(&identity.hub, &identity.key)?;
            let key = self.hub_key(&identity.hub, &signed.proof.signer)?;
            let value: EpochDescriptor = open(&signed.proof, ProofPurpose::EpochHandoverV1, key)?;
            if value.scope.hub != self.hub {
                return Err(Error::Denied("epoch proof belongs to another hub".into()));
            }
            validation::epoch(&value)?;
            Ok(value)
        })
    }
    fn rotation(&self, signed: &SignedStatement, previous: &HubSigningIdentity) -> Result<HubKeyRotation, Error> {
        self.measured(|| {
            let key = self.hub_key(&previous.hub, &previous.key)?;
            let value: HubKeyRotation = open(signed, ProofPurpose::HubKeyRotationV1, key)?;
            if value.hub != self.hub
                || value.previous != previous.key
                || value.next == value.previous
                || previous.revision.0.checked_add(1) != Some(value.revision.0)
            {
                return Err(Error::Denied("rotation does not continue the pinned key chain".into()));
            }
            Ok(value)
        })
    }
}
fn device_key(device: &DeviceId) -> PublicKey {
    let text = &device.as_str()[2..];
    PublicKey::from_bytes(std::array::from_fn(|index| {
        u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).expect("validated device identity")
    }))
}
