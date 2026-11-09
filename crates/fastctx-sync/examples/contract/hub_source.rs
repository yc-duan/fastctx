use fastctx_sync::{
    catalog::{SourceBatch, SourceRecord},
    cursor::{HoldingPeriod, Sequence},
    identity::{Digest, OperationId, Producer, ProducerEpoch, StreamId},
    manifest::{BlobId, ContentBlock, DigestAlgorithm},
    proof::{Bytes, ProofPurpose, Signature, SignedSourceBatch, SignedStatement, SigningKey},
    transfer::Message,
};

pub fn sample(holding: &HoldingPeriod) -> Result<(Message, SourceBatch), serde_json::Error> {
    let batch = SourceBatch {
        format: 1,
        scope: holding.scope.clone(),
        stream: StreamId::from_bytes([42; 32]),
        producer: Producer::Hub,
        epoch: ProducerEpoch::from_bytes([43; 32]),
        operation: OperationId::from_bytes([44; 32]),
        kind: "account.directory".into(),
        schema: 1,
        after: Sequence(0),
        through: Sequence(1),
        records: vec![SourceRecord {
            sequence: Sequence(1),
            operation: OperationId::from_bytes([44; 32]),
            kind: "account.directory".into(),
            schema: 1,
            payload: ContentBlock {
                id: BlobId {
                    domain: holding.scope.clone(),
                    algorithm: DigestAlgorithm::Sha256,
                    digest: Digest::from_bytes([45; 32]),
                },
                decoded_bytes: 1,
            },
            roots: vec![],
            causes: vec![],
        }],
    };
    let proof = SignedStatement {
        purpose: ProofPurpose::HubSourceBatchV1,
        signer: SigningKey([35; 32]),
        data: Bytes(serde_json::to_vec(&batch)?),
        signature: Signature([0; 64]),
    };
    Ok((Message::Source { holding: holding.clone(), batch: SignedSourceBatch { proof } }, batch))
}
