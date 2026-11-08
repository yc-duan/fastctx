use super::{domain, encode, validation};
use crate::sign::SecretKey;
use fastctx_sync::{
    catalog::{Authority, SourceBatch},
    exposure::{ExposureReceipt, SignedExposureReceipt},
    identity::{DeviceId, HubId},
    metrics::{Latency, Metrics},
    proof::{
        Bytes, CommittedFragment, ConfirmedEpoch, EpochDescriptor, HubKeyRotation, ProducerRelease, ProofPurpose,
        ProofSigner, Signature, SignedAcceptanceFragment, SignedSourceBatch, SignedStatement, SigningKey,
    },
    store::Error,
};
use std::{sync::Arc, time::Instant};

#[derive(Clone, Copy)]
enum Role {
    Hub,
    Device,
}

/// Keys are loaded before constructing this adapter. It never reads files and
/// deliberately has no Debug or serialization implementation. SecretKey's
/// ed25519-dalek key zeroizes its seed when the last owner releases it.
pub struct IdentitySigner {
    hub: HubId,
    role: Role,
    key: Arc<SecretKey>,
    metrics: Arc<dyn Metrics>,
}
impl IdentitySigner {
    pub fn hub(hub: HubId, key: Arc<SecretKey>, metrics: Arc<dyn Metrics>) -> Self {
        Self { hub, role: Role::Hub, key, metrics }
    }
    pub fn device(hub: HubId, key: Arc<SecretKey>, metrics: Arc<dyn Metrics>) -> Self {
        Self { hub, role: Role::Device, key, metrics }
    }
    fn require_hub(&self, hub: &HubId) -> Result<(), Error> {
        if hub != &self.hub {
            return Err(Error::Denied("proof belongs to a different hub".into()));
        }
        Ok(())
    }
    fn device_id(&self) -> DeviceId {
        DeviceId::from_bytes(*self.key.public().as_bytes())
    }
    fn require_role(&self, hub: bool) -> Result<(), Error> {
        if matches!(self.role, Role::Hub) != hub {
            return Err(Error::Denied("this key role cannot sign that statement".into()));
        }
        Ok(())
    }
    fn signed(&self, purpose: ProofPurpose, data: Vec<u8>) -> SignedStatement {
        let began = Instant::now();
        let signature = self.key.sign(domain(purpose), &data);
        self.metrics.elapsed(Latency::Sign, u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX));
        SignedStatement {
            purpose,
            signer: SigningKey(*self.key.public().as_bytes()),
            data: Bytes(data),
            signature: Signature(signature.to_bytes()),
        }
    }
}
impl ProofSigner for IdentitySigner {
    fn accepted(&self, value: &CommittedFragment) -> Result<SignedAcceptanceFragment, Error> {
        self.require_hub(&value.fragment.binding.hub)?;
        match &value.fragment.binding.authority {
            Authority::Hub => self.require_role(true)?,
            Authority::Device { device, .. } => {
                self.require_role(false)?;
                if device != &self.device_id() {
                    return Err(Error::Denied("accepted result names another device authority".into()));
                }
                validation::device_kind(&value.fragment.kind)?;
            }
        }
        let data = encode(value)?;
        value.validate().map_err(Error::Invalid)?;
        Ok(SignedAcceptanceFragment { proof: self.signed(ProofPurpose::AcceptedOperationV1, data) })
    }
    fn source(&self, value: &SourceBatch) -> Result<SignedSourceBatch, Error> {
        self.require_role(false)?;
        self.require_hub(&value.scope.hub)?;
        if value.producer != self.device_id() {
            return Err(Error::Denied("source batch names another producer".into()));
        }
        let data = encode(value)?;
        validation::source(value)?;
        Ok(SignedSourceBatch { proof: self.signed(ProofPurpose::SourceBatchV1, data) })
    }
    fn epoch(&self, value: &EpochDescriptor) -> Result<ConfirmedEpoch, Error> {
        self.require_role(true)?;
        self.require_hub(&value.scope.hub)?;
        let data = encode(value)?;
        validation::epoch(value)?;
        Ok(ConfirmedEpoch { proof: self.signed(ProofPurpose::EpochHandoverV1, data) })
    }
    fn release(&self, value: &ProducerRelease) -> Result<SignedStatement, Error> {
        self.require_role(false)?;
        self.require_hub(&value.predecessor.scope.hub)?;
        let data = encode(value)?;
        Ok(self.signed(ProofPurpose::ProducerReleaseV1, data))
    }
    fn exposure(&self, value: &ExposureReceipt) -> Result<SignedExposureReceipt, Error> {
        self.require_role(false)?;
        self.require_hub(&value.hub)?;
        if value.machine != self.device_id() {
            return Err(Error::Denied("exposure receipt names another machine".into()));
        }
        Ok(SignedExposureReceipt { proof: self.signed(ProofPurpose::ExposureReceiptV1, encode(value)?) })
    }
    fn rotation(&self, value: &HubKeyRotation) -> Result<SignedStatement, Error> {
        self.require_role(true)?;
        self.require_hub(&value.hub)?;
        if value.previous.0 != *self.key.public().as_bytes() || value.next == value.previous || value.revision.0 == 0 {
            return Err(Error::Denied("rotation differs from the signing identity".into()));
        }
        Ok(self.signed(ProofPurpose::HubKeyRotationV1, encode(value)?))
    }
}
