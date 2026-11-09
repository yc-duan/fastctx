//! Typed placeholder signatures keep the library lifecycle independent of net.
use super::*;
use fastctx_sync::proof::{
    Bytes, CommittedFragment, HubKeyRotation, ProducerRelease, ProofPurpose, ProofSigner, ProofVerifier, Signature,
    SignedAcceptanceFragment, SignedStatement, SigningKey,
};

pub(super) struct Signer(pub bool);
impl ProofSigner for Signer {
    fn accepted(&self, _: &CommittedFragment) -> std::result::Result<SignedAcceptanceFragment, Error> {
        Err(Error::Unavailable("unused drill signing purpose".into()))
    }
    fn source(&self, batch: &SourceBatch) -> std::result::Result<SignedSourceBatch, Error> {
        Ok(SignedSourceBatch {
            proof: statement(
                if self.0 { ProofPurpose::HubSourceBatchV1 } else { ProofPurpose::SourceBatchV1 },
                if self.0 { 9 } else { 3 },
                batch,
            )?,
        })
    }
    fn epoch(&self, value: &EpochDescriptor) -> std::result::Result<ConfirmedEpoch, Error> {
        assert!(self.0);
        Ok(ConfirmedEpoch { proof: statement(ProofPurpose::EpochHandoverV1, 9, value)? })
    }
    fn release(&self, _: &ProducerRelease) -> std::result::Result<SignedStatement, Error> {
        Err(Error::Unavailable("unused drill signing purpose".into()))
    }
    fn exposure(
        &self,
        _: &fastctx_sync::exposure::ExposureReceipt,
    ) -> std::result::Result<fastctx_sync::exposure::SignedExposureReceipt, Error> {
        Err(Error::Unavailable("unused drill signing purpose".into()))
    }
    fn rotation(&self, _: &HubKeyRotation) -> std::result::Result<SignedStatement, Error> {
        Err(Error::Unavailable("unused drill signing purpose".into()))
    }
}
fn statement(
    purpose: ProofPurpose,
    key: u8,
    value: &impl serde::Serialize,
) -> std::result::Result<SignedStatement, Error> {
    Ok(SignedStatement {
        purpose,
        signer: SigningKey([key; 32]),
        data: Bytes(serde_json::to_vec(value).map_err(|e| Error::Invalid(e.to_string()))?),
        signature: Signature([1; 64]),
    })
}
pub(super) struct Verifier;
impl ProofVerifier for Verifier {
    fn accepted(
        &self,
        _: &SignedAcceptanceFragment,
        _: &fastctx_sync::catalog::Authority,
    ) -> std::result::Result<CommittedFragment, Error> {
        Err(Error::Unavailable("unused drill verifying purpose".into()))
    }
    fn source(&self, signed: &SignedSourceBatch, epoch: &EpochDescriptor) -> std::result::Result<SourceBatch, Error> {
        let value: SourceBatch =
            serde_json::from_slice(&signed.proof.data.0).map_err(|e| Error::Invalid(e.to_string()))?;
        let (purpose, key) = match epoch.producer {
            Producer::Hub => (ProofPurpose::HubSourceBatchV1, 9),
            _ => (ProofPurpose::SourceBatchV1, 3),
        };
        if signed.proof.purpose != purpose
            || signed.proof.signer.0 != [key; 32]
            || signed.proof.signature.0 != [1; 64]
            || value.scope != epoch.scope
            || value.stream != epoch.stream
            || value.epoch != epoch.epoch
            || value.producer != epoch.producer
            || value.kind != epoch.kind
            || value.schema != epoch.schema
        {
            return Err(Error::Denied("drill source differs from its evidence binding".into()));
        }
        value.validate_shape().map_err(Error::Invalid)?;
        Ok(value)
    }
    fn epoch(
        &self,
        signed: &ConfirmedEpoch,
        identity: &HubSigningIdentity,
    ) -> std::result::Result<EpochDescriptor, Error> {
        if signed.proof.purpose != ProofPurpose::EpochHandoverV1
            || signed.proof.signer != identity.key
            || signed.proof.signature.0 != [1; 64]
        {
            return Err(Error::Denied("drill epoch has no trusted hub evidence".into()));
        }
        let value: EpochDescriptor =
            serde_json::from_slice(&signed.proof.data.0).map_err(|e| Error::Invalid(e.to_string()))?;
        if value.scope.hub != identity.hub {
            return Err(Error::Denied("drill epoch names another hub".into()));
        }
        Ok(value)
    }
    fn rotation(&self, _: &SignedStatement, _: &HubSigningIdentity) -> std::result::Result<HubKeyRotation, Error> {
        Err(Error::Unavailable("unused drill verifying purpose".into()))
    }
}
pub(super) struct Policy(pub AccountId);
impl Policy {
    fn decide(&self, c: &AccessContext) -> Decision {
        if matches!(&c.principal, Principal::LocalService)
            || matches!(&c.principal,Principal::Account{account,device} if account==&self.0&&device==&DeviceId::from_bytes([3;32]))
        {
            Decision::Allowed { revision: c.observed_acl }
        } else {
            Decision::Denied { revision: c.observed_acl, reason: "outside the drill account".into() }
        }
    }
}
impl AccessPolicy for Policy {
    fn can_read(&self, c: &AccessContext, _: RegisteredKind) -> Decision {
        self.decide(c)
    }
    fn can_read_object(&self, c: &AccessContext, _: RegisteredKind, _: &ObjectKey) -> Decision {
        self.decide(c)
    }
    fn can_write(&self, c: &AccessContext, _: RegisteredKind) -> Decision {
        self.decide(c)
    }
    fn can_keep(&self, c: &AccessContext, _: RegisteredKind, _: &ObjectKey, purpose: &HoldingUse) -> Decision {
        if let HoldingUse::RuntimeSource { host } = purpose
            && (host.account != self.0 || host.host != DeviceId::from_bytes([3; 32]))
        {
            return Decision::Unavailable;
        }
        self.decide(c)
    }
    fn can_execute(&self, c: &AccessContext, _: &DeviceId) -> Decision {
        Decision::Denied { revision: c.observed_acl, reason: "no execution in the drill".into() }
    }
}
pub(super) struct Gate(pub AtomicBool);
impl SourceAuthorizer for Gate {
    fn kind(&self) -> &'static str {
        KIND
    }
    fn authorize(&self, _: PreparedSourceChange<'_>) -> Decision {
        Decision::Allowed { revision: Revision(1) }
    }
    fn authorize_received(&self, change: ReceivedSourceBatch<'_>) -> Decision {
        assert_eq!(change.payloads.len(), change.batch.records.len());
        if self.0.load(Ordering::Acquire) {
            Decision::Denied { revision: Revision(1), reason: "hub semantic policy denies this source".into() }
        } else {
            Decision::Allowed { revision: change.custody.context.observed_acl }
        }
    }
}
pub(super) fn identity() -> HubSigningIdentity {
    HubSigningIdentity { hub: hub(), key: SigningKey([9; 32]), revision: Revision(1), rotations: Vec::new() }
}
pub(super) fn entropy(seed: u8) -> fastctx_sync::store::control::Entropy {
    let next = Arc::new(AtomicUsize::new(usize::from(seed)));
    Arc::new(move || {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&(next.fetch_add(1, Ordering::AcqRel) as u64).to_be_bytes());
        Ok(bytes)
    })
}
