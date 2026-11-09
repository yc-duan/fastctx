//! Source receipts distinguish accepted facts from the completeness of their bodies.
use fastctx_types::sync::{
    catalog::{Authority, HeadEvidence, Lifecycle, ObjectHead, SourceBatch},
    cursor::{Completeness, Revision, ScopeCursor, Sequence, StreamCursor, VersionRef},
    identity::{AccountId, AuthorityEpoch, ObjectId, ObjectKey, Producer},
    proof::{
        AcceptedActor, Bytes, ConfirmedEpoch, EpochDescriptor, HandoverBasis, ProofPurpose, Signature,
        SignedSourceBatch, SignedStatement, SigningKey,
    },
    source_acceptance::{SourceAccepted, SourceReceipt},
    transfer::Message,
};

pub fn sample(
    cursor: &ScopeCursor,
    source: &SourceBatch,
    signed: &SignedSourceBatch,
) -> Result<Vec<Message>, Box<dyn std::error::Error>> {
    let Producer::Device { device } = &source.producer else {
        return Err("source receipt sample needs a device".into());
    };
    let descriptor = EpochDescriptor {
        scope: source.scope.clone(),
        stream: source.stream.clone(),
        kind: source.kind.clone(),
        schema: source.schema,
        epoch: source.epoch.clone(),
        producer: source.producer.clone(),
        predecessor: None,
        closed_through: None,
        operation: source.operation.clone(),
        control_sequence: Sequence(1),
        basis: HandoverBasis::Initial { registration: source.operation.clone() },
    };
    let epoch = ConfirmedEpoch {
        proof: SignedStatement {
            purpose: ProofPurpose::EpochHandoverV1,
            signer: SigningKey([9; 32]),
            data: Bytes(serde_json::to_vec(&descriptor)?),
            signature: Signature([0; 64]),
        },
    };
    let mut prefix = fastctx_types::sync::feed::stream_genesis(&epoch.proof.id());
    for record in &source.records {
        prefix = fastctx_types::sync::feed::stream_advance(&prefix, record)?.1;
    }
    let version = VersionRef { authority_epoch: AuthorityEpoch::from_bytes([31; 16]), revision: Revision(1) };
    let head = ObjectHead {
        key: ObjectKey { scope: source.scope.clone(), object: ObjectId::from_bytes([42; 32]) },
        kind: source.kind.clone(),
        schema: source.schema,
        acl_revision: cursor.acl_revision,
        authority: Authority::Device { device: device.clone(), epoch: source.epoch.clone() },
        evidence: HeadEvidence::Stream {
            epoch: epoch.proof.id(),
            through: source.through,
            prefix_digest: prefix.clone(),
        },
        revision: version.clone(),
        current_version: version,
        content: None,
        history: None,
        lifecycle: Lifecycle::Live,
        completeness: Completeness::MetadataOnly,
    };
    let receipt = Box::new(SourceReceipt {
        operation: source.operation.clone(),
        actor: AcceptedActor::Account { account: AccountId::from_bytes([7; 32]), device: device.clone() },
        proof: signed.proof.id(),
        epoch,
        stream: StreamCursor {
            scope: source.scope.clone(),
            stream: source.stream.clone(),
            epoch: source.epoch.clone(),
            committed: source.through,
        },
        prefix_digest: prefix,
        head,
        scope: ScopeCursor { view: None, ..cursor.clone() },
    });
    Ok(vec![
        Message::SourceResult {
            result: SourceAccepted::Accepted { receipt: receipt.clone(), content_complete: false },
        },
        Message::SourceResult { result: SourceAccepted::Accepted { receipt, content_complete: true } },
        Message::SourceResult { result: SourceAccepted::AwaitingEpoch { proof: signed.proof.id() } },
    ])
}
