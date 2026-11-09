//! Catalog evidence is explicit even in samples with deliberately invalid signatures.
use fastctx_types::sync::{
    catalog::{Authority, CatalogPage, HeadEvidence, ObjectHead, SourceBatch},
    cursor::{Completeness, HoldingPeriod, ScopeCursor, SnapshotFence},
    identity::{Digest, ObjectId, ObjectKey, Producer},
    manifest::{BlobId, DigestAlgorithm, ManifestRoot},
    proof::{CommittedFragment, SignedAcceptanceFragment},
    transfer::Message,
};
pub fn sample(
    holding: &HoldingPeriod,
    cursor: &ScopeCursor,
    proof: &SignedAcceptanceFragment,
    accepted: &CommittedFragment,
    source: &SourceBatch,
) -> Result<Message, Box<dyn std::error::Error>> {
    let object = &accepted.fragment.objects[0];
    let head = ObjectHead {
        key: object.key.clone(),
        kind: object.kind.clone(),
        schema: object.schema,
        acl_revision: holding.acl_revision,
        authority: accepted.fragment.binding.authority.clone(),
        evidence: HeadEvidence::Accepted { proof: proof.proof.id() },
        revision: object.allocated.clone(),
        current_version: object.current_version.clone(),
        content: object.current_content.clone(),
        history: object.history.clone(),
        lifecycle: object.lifecycle,
        completeness: Completeness::MetadataOnly,
    };
    let epoch = Digest::from_bytes([41; 32]);
    let mut prefix = fastctx_types::sync::feed::stream_genesis(&epoch);
    for record in &source.records {
        prefix = fastctx_types::sync::feed::stream_advance(&prefix, record)?.1;
    }
    let authority = match &source.producer {
        Producer::Hub => Authority::Hub,
        Producer::Device { device } => Authority::Device { device: device.clone(), epoch: source.epoch.clone() },
    };
    let stream = ObjectHead {
        key: ObjectKey { scope: source.scope.clone(), object: ObjectId::from_bytes([42; 32]) },
        kind: source.kind.clone(),
        schema: source.schema,
        authority,
        evidence: HeadEvidence::Stream { epoch, through: source.through, prefix_digest: prefix },
        content: None,
        history: None,
        ..head.clone()
    };
    Ok(Message::CatalogPage {
        holding: holding.clone(),
        page: CatalogPage {
            fence: SnapshotFence {
                cursor: cursor.clone(),
                root: ManifestRoot(BlobId {
                    domain: cursor.scope.clone(),
                    algorithm: DigestAlgorithm::Sha256,
                    digest: Digest::from_bytes([43; 32]),
                }),
            },
            objects: vec![head, stream],
            proofs: vec![proof.clone()],
            next: None,
        },
    })
}
