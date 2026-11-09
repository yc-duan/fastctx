//! Draft local/confirmed fences never substitute zeroed proof fields for absence.
use fastctx_types::sync::{
    cursor::{LocalSequence, Revision, Sequence, StreamCursor},
    identity::{Digest, ObjectId, ObjectKey, ProducerEpoch, RootLeaseId, ScopeKey, StreamId},
    manifest::{BlobId, DigestAlgorithm, ManifestRoot},
    progress::StreamProgress,
    timeline::{TimelineFence, TimelinePage, TimelineResult},
};

pub fn sample(scope: ScopeKey) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let cursor = StreamCursor {
        scope: scope.clone(),
        stream: StreamId::from_bytes([60; 32]),
        epoch: ProducerEpoch::from_bytes([61; 32]),
        committed: Sequence(3),
    };
    let pending = StreamProgress::LocalPending { cursor: cursor.clone() };
    let confirmed = StreamProgress::Confirmed {
        cursor,
        epoch_proof: Digest::from_bytes([62; 32]),
        prefix_digest: Digest::from_bytes([63; 32]),
    };
    let local = serde_json::to_value(&pending)?;
    assert!(local.get("epoch_proof").is_none() && local.get("prefix_digest").is_none());
    let fence = TimelineFence {
        conversation: ObjectKey { scope: scope.clone(), object: ObjectId::from_bytes([64; 32]) },
        projection: Revision(1),
        through: pending.clone(),
        applied: LocalSequence(7),
        local: LocalSequence(11),
        root: ManifestRoot(BlobId {
            domain: scope,
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::from_bytes([65; 32]),
        }),
        lease: RootLeaseId::from_bytes([66; 32]),
    };
    let page = TimelineResult::Page { page: Box::new(TimelinePage { fence, items: vec![], older: None, newer: None }) };
    let encoded = serde_json::to_vec(&page)?;
    assert_eq!(serde_json::from_slice::<TimelineResult>(&encoded)?, page);
    Ok(serde_json::json!({"evidence_status":"shape_only_not_validated", "local_pending":pending,
        "confirmed":confirmed,"page":page,"projection_pending":TimelineResult::Pending{progress:None},
        "reanchor":TimelineResult::ReanchorNeeded}))
}
