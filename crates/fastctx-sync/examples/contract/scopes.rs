//! Scope discovery samples contain deliberately invalid signature placeholders.
use fastctx_sync::{
    catalog::{Lifecycle, SourceBatch, SourceRecord},
    cursor::{Revision, ScopeCursor, Sequence, VersionRef},
    identity::{
        AccountId, AuthorityEpoch, DeviceId, Digest, ObjectId, OperationId, Producer, ProducerEpoch, ScopeId, ScopeKey,
        StreamId,
    },
    manifest::{BlobId, ContentBlock, DigestAlgorithm},
    proof::{
        Bytes, ConfirmedEpoch, EpochDescriptor, HandoverBasis, ProofPurpose, Signature, SignedSourceBatch,
        SignedStatement, SigningKey,
    },
    scope::{
        AccountIdentity, DiscoveryFence, ScopeDescriptor, ScopeDiscoveryRead, ScopeOffer, ScopeOwner, ScopeProof,
        ScopeSubject, ScopeVisibility, ScopesPage,
    },
    transfer::{KindVersion, Message},
};

pub fn sample(cursor: &ScopeCursor, device: DeviceId) -> Result<Vec<Message>, Box<dyn std::error::Error>> {
    let descriptor = ScopeDescriptor {
        format: 1,
        scope: cursor.scope.clone(),
        owner: ScopeOwner::Account {
            identity: AccountIdentity {
                id: AccountId::from_bytes([34; 32]),
                name: "reader".parse()?,
                opened: "2026-01-01T00:00:00Z".parse()?,
            },
        },
        visibility: ScopeVisibility::Current,
        subject: ScopeSubject::Conversation {
            conversation: ObjectId::from_bytes([40; 32]),
            shared_on: None,
            born_seal: None,
        },
        revision: VersionRef { authority_epoch: AuthorityEpoch::from_bytes([31; 16]), revision: Revision(1) },
        acl_revision: cursor.acl_revision,
        lifecycle: Lifecycle::Live,
    };
    let payload = serde_json::to_vec(&descriptor)?;
    let directory = ScopeKey { hub: cursor.scope.hub.clone(), scope: ScopeId::from_bytes([50; 32]) };
    let operation = OperationId::from_bytes([51; 32]);
    let epoch = EpochDescriptor {
        scope: directory.clone(),
        stream: StreamId::from_bytes([52; 32]),
        kind: "account.directory".into(),
        schema: 1,
        epoch: ProducerEpoch::from_bytes([53; 32]),
        producer: Producer::Hub,
        predecessor: None,
        closed_through: None,
        operation: operation.clone(),
        control_sequence: Sequence(1),
        basis: HandoverBasis::Initial { registration: operation.clone() },
    };
    let batch = SourceBatch {
        format: 1,
        scope: directory.clone(),
        stream: epoch.stream.clone(),
        epoch: epoch.epoch.clone(),
        producer: Producer::Hub,
        kind: epoch.kind.clone(),
        schema: 1,
        after: Sequence(0),
        through: Sequence(1),
        records: vec![SourceRecord {
            sequence: Sequence(1),
            operation,
            kind: epoch.kind.clone(),
            schema: 1,
            causes: vec![],
            payload: ContentBlock {
                id: BlobId {
                    domain: directory.clone(),
                    algorithm: DigestAlgorithm::Sha256,
                    digest: Digest::of(&payload),
                },
                decoded_bytes: payload.len() as u32,
            },
        }],
    };
    let placeholder = |purpose, data| SignedStatement {
        purpose,
        signer: SigningKey([35; 32]),
        data: Bytes(data),
        signature: Signature([0; 64]),
    };
    let proof = ScopeProof {
        epoch: ConfirmedEpoch { proof: placeholder(ProofPurpose::EpochHandoverV1, serde_json::to_vec(&epoch)?) },
        batch: SignedSourceBatch { proof: placeholder(ProofPurpose::HubSourceBatchV1, serde_json::to_vec(&batch)?) },
        payload: Bytes(payload),
    };
    Ok(vec![
        Message::GetScopes {
            request: ScopeDiscoveryRead { cursor: None, directories: vec![], max_items: 100, max_bytes: 262144 },
        },
        Message::ScopesPage {
            page: ScopesPage {
                fence: DiscoveryFence {
                    recipient: device,
                    view: cursor.view.clone().expect("sample view"),
                    snapshot: Digest::from_bytes([54; 32]),
                },
                entries: vec![ScopeOffer {
                    binding: proof,
                    cursor: cursor.clone(),
                    kinds: vec![KindVersion { kind: "conversation.timeline".into(), schema: 1 }],
                }],
                directories: vec![ScopeCursor { scope: directory, through: Sequence(1), ..cursor.clone() }],
                next: None,
            },
        },
    ])
}
