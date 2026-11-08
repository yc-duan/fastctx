//! Invalid signature bytes demonstrate the acceptance shape without live keys.

use fastctx_types::sync::{
    acceptance::{
        AcceptanceBinding, AcceptanceFragment, AcceptedActor, CommittedFragment, SignedAcceptanceFragment,
        result_digest,
    },
    catalog::{Authority, Lifecycle},
    cursor::{Revision, VersionRef},
    identity::{AccountId, AuthorityEpoch, DeviceId, Digest, ObjectId, ObjectKey, OperationId, ScopeKey},
    manifest::{BlobId, DigestAlgorithm},
    mutation::{Acceptance, AcceptedObject},
    proof::{Bytes, ProofPurpose, Signature, SignedStatement, SigningKey},
};

pub fn sample(
    scope: ScopeKey,
    device: DeviceId,
) -> Result<(SignedAcceptanceFragment, CommittedFragment), Box<dyn std::error::Error>> {
    let epoch = AuthorityEpoch::from_bytes([31; 16]);
    let version = VersionRef { authority_epoch: epoch.clone(), revision: Revision(8) };
    let content =
        BlobId { domain: scope.clone(), algorithm: DigestAlgorithm::Sha256, digest: Digest::from_bytes([24; 32]) };
    let fragment = AcceptanceFragment {
        binding: AcceptanceBinding {
            format: 1,
            hub: scope.hub.clone(),
            authority: Authority::Hub,
            authority_epoch: epoch,
            operation: OperationId::from_bytes([21; 32]),
            request_hash: Digest::from_bytes([33; 32]),
            actor: AcceptedActor::Account { account: AccountId::from_bytes([34; 32]), device },
        },
        scope: scope.scope.clone(),
        kind: "account.settings".into(),
        schema: 1,
        objects: vec![AcceptedObject {
            key: ObjectKey { scope: scope.clone(), object: ObjectId::from_bytes([22; 32]) },
            kind: "account.settings".into(),
            schema: 1,
            allocated: version.clone(),
            parents: vec![],
            content: Some(content.clone()),
            current_version: version,
            current_content: Some(content),
            history: None,
            lifecycle: Lifecycle::Live,
            acceptance: Acceptance::Applied,
        }],
    };
    let fragment_digest = fragment.digest()?;
    let result_digest = result_digest(&[(fragment.key(), fragment_digest.clone())])?;
    let committed = CommittedFragment { fragment, fragment_digest, result_digest, index: 0, count: 1 };
    let signed = SignedAcceptanceFragment {
        proof: SignedStatement {
            purpose: ProofPurpose::AcceptedOperationV1,
            signer: SigningKey([35; 32]),
            data: Bytes(serde_json::to_vec(&committed)?),
            signature: Signature([0; 64]),
        },
    };
    Ok((signed, committed))
}
