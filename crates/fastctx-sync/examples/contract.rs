//! Emits draft wire shapes. Zeroed signatures are deliberately not valid proofs.

#[path = "contract/acceptance.rs"]
mod acceptance;
#[path = "contract/catalog.rs"]
mod catalog;
#[path = "contract/hub_source.rs"]
mod hub_source;
#[path = "contract/indexes.rs"]
mod indexes;
#[path = "contract/manifests.rs"]
mod manifests;
#[path = "contract/scopes.rs"]
mod scopes;
#[path = "contract/transfer.rs"]
mod transfer;

use fastctx_sync::{
    catalog::{ChangesPage, FeedEntry, SourceBatch, SourceRecord},
    cursor::{ByteCount, DurableAck, Holder, HoldingPeriod, Revision, ScopeCursor, Sequence, StreamCursor, VersionRef},
    identity::{
        AuthorityEpoch, DeviceId, Digest, FeedEpoch, HubId, ObjectId, ObjectKey, OperationId, Producer, ProducerEpoch,
        ScopeId, ScopeKey, StreamId,
    },
    manifest::{BlobId, ContentBlock, ContentRef, DigestAlgorithm, ManifestRoot},
    proof::{Bytes, ProofPurpose, Signature, SignedSourceBatch, SignedStatement, SigningKey},
    store::mutation::{BaseVersion, FieldEdit, OriginalIntent, ProposedObject, Submission, SubmissionMode},
    timeline::{StreamPosition, StreamRead, TimelineChange, TimelineItem, TimelinePosition},
    transfer::{Capabilities, KindVersion, Message, ViewDefinition},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scope = ScopeKey { hub: HubId::from_bytes([1; 8]), scope: ScopeId::from_bytes([2; 32]) };
    let device = DeviceId::from_bytes([3; 32]);
    let kind = KindVersion { kind: "conversation.timeline".into(), schema: 1 };
    let view = ViewDefinition {
        scope: scope.clone(),
        acl_revision: Revision(1),
        projection_schema: 1,
        kinds: vec![kind.clone()],
    };
    let cursor = ScopeCursor {
        scope: scope.clone(),
        epoch: FeedEpoch::from_bytes([4; 32]),
        acl_revision: Revision(1),
        view: Some(view.id()?),
        through: Sequence(9_007_199_254_740_993),
        prefix_digest: Some(Digest::from_bytes([30; 32])),
    };
    let holding = HoldingPeriod {
        holder: Holder::Device { device: device.clone() },
        scope: scope.clone(),
        acl_revision: Revision(1),
        publication_epoch: Revision(1),
    };
    let stream = StreamCursor {
        scope: scope.clone(),
        stream: StreamId::from_bytes([5; 32]),
        epoch: ProducerEpoch::from_bytes([6; 32]),
        committed: Sequence(4),
    };
    let blob = |byte| BlobId {
        domain: scope.clone(),
        algorithm: DigestAlgorithm::Sha256,
        digest: Digest::from_bytes([byte; 32]),
    };
    let content = |byte| ContentRef::Single { block: blob(byte), decoded_bytes: 1 };
    let records = (1_u8..=4)
        .map(|n| SourceRecord {
            sequence: Sequence(n.into()),
            operation: OperationId::from_bytes([n; 32]),
            kind: kind.kind.clone(),
            schema: 1,
            payload: ContentBlock { id: blob(n), decoded_bytes: 1 },
            causes: vec![],
        })
        .collect();
    let batch = SourceBatch {
        format: 1,
        scope: scope.clone(),
        stream: stream.stream.clone(),
        producer: Producer::Device { device: device.clone() },
        epoch: stream.epoch.clone(),
        kind: kind.kind.clone(),
        schema: 1,
        after: Sequence(0),
        through: Sequence(4),
        records,
    };
    let signed = SignedSourceBatch {
        proof: SignedStatement {
            purpose: ProofPurpose::SourceBatchV1,
            signer: SigningKey([3; 32]),
            data: Bytes(serde_json::to_vec(&batch)?),
            signature: Signature([0; 64]),
        },
    };
    let mut timeline = Vec::new();
    for (sequence, item, revision) in [(1_u8, 1_u8, 1_u64), (2, 2, 1), (3, 3, 1), (4, 2, 2)] {
        timeline.push(TimelineChange::Put {
            item: TimelineItem {
                id: ObjectId::from_bytes([item; 32]),
                revision: Revision(revision),
                turn: ObjectId::from_bytes([20; 32]),
                parent: None,
                position: TimelinePosition {
                    turn: Sequence(1),
                    display_ancestry: vec![Sequence(item.into())],
                    accepted: Sequence(item.into()),
                    item: ObjectId::from_bytes([item; 32]),
                },
                body: content(sequence),
            },
        });
    }
    let (accepted, accepted_binding) = acceptance::sample(scope.clone(), device.clone())?;
    let (index_messages, index_payloads) =
        indexes::sample(&holding, &cursor, &accepted, &accepted_binding, &signed, &batch, &timeline)?;
    let submission = Submission {
        operation: OperationId::from_bytes([21; 32]),
        origin: device,
        mode: SubmissionMode::Offline,
        objects: vec![ProposedObject {
            key: ObjectKey { scope: scope.clone(), object: ObjectId::from_bytes([22; 32]) },
            kind: "account.settings".into(),
            observed_acl: Revision(1),
            base: BaseVersion::Version {
                state_revision: VersionRef {
                    authority_epoch: AuthorityEpoch::from_bytes([31; 16]),
                    revision: Revision(7),
                },
                content_version: VersionRef {
                    authority_epoch: AuthorityEpoch::from_bytes([31; 16]),
                    revision: Revision(7),
                },
                root: Some(content(23)),
            },
            before: Some(content(23)),
            candidate: Some(content(24)),
            predecessor: None,
            original: OriginalIntent::Fields {
                edits: vec![FieldEdit::Set { path: vec!["theme".into()], value: serde_json::json!("dark") }],
            },
            resolves: vec![],
        }],
    };
    submission.validate_bounds()?;
    let mut messages = vec![
        Message::Hello {
            capabilities: Capabilities {
                protocols: vec!["fastctx-sync/v1".into(), "future-protocol".into()],
                kinds: vec![kind.clone()],
                envelopes: vec!["plain/v1".into()],
                encodings: vec!["raw".into(), "zstd".into(), "future-encoding".into()],
                receive_window: ByteCount(4 * 1024 * 1024),
                max_page_bytes: 256 * 1024,
                exposure_cleanup: vec!["physical/v1".into()],
                hub_signing: None,
                transports: vec!["asset-transfer/v1".into(), "future-transport".into()],
            },
        },
        Message::Cursors { entries: vec![cursor.clone()], more: false },
        Message::Catalog { holding: holding.clone(), cursor: None, scope: scope.clone(), kinds: vec![kind] },
        Message::Changes { holding: holding.clone(), after: cursor.clone(), max_bytes: 256 * 1024 },
        Message::ChangesPage {
            holding: holding.clone(),
            page: ChangesPage {
                after: cursor.clone(),
                through: ScopeCursor {
                    through: Sequence(cursor.through.0 + 2),
                    prefix_digest: Some(Digest::from_bytes([32; 32])),
                    ..cursor.clone()
                },
                entries: vec![FeedEntry::Skipped {
                    after: cursor.through,
                    through: Sequence(cursor.through.0 + 2),
                    view: view.id()?,
                    prefix_digest: Digest::from_bytes([32; 32]),
                }],
                proofs: vec![],
                more: false,
            },
        },
        Message::Source { holding: holding.clone(), batch: signed },
        Message::StreamRange {
            holding: holding.clone(),
            request: StreamRead {
                stream,
                position: StreamPosition::Latest,
                through: Some(Sequence(4)),
                max_records: 100,
                max_bytes: 256 * 1024,
            },
        },
        Message::Submit { request: submission },
        Message::RestoreAccepted { proof: accepted.clone() },
        Message::GetManifest { holding: holding.clone(), root: ManifestRoot(blob(25)) },
        Message::GetDescriptor { holding: holding.clone(), blob: blob(1) },
        Message::Acknowledge { durable: DurableAck::Catalog { holding: holding.clone(), cursor: cursor.clone() } },
        Message::Revoked { holding: holding.clone() },
    ];
    messages.push(catalog::sample(&holding, &cursor, &accepted, &accepted_binding, &batch)?);
    messages.extend(manifests::sample(&holding)?);
    messages.extend(index_messages);
    messages.extend(scopes::sample(&cursor, DeviceId::from_bytes([3; 32]))?);
    let (hub_message, hub_batch) = hub_source::sample(&holding)?;
    messages.push(hub_message);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "signature_status": "shape_only_not_validated",
            "messages": messages, "decoded_source_binding": batch, "timeline_payload_shapes": timeline,
            "decoded_acceptance_binding": accepted_binding,
            "decoded_hub_source_binding": hub_batch,
            "decoded_index_payloads": index_payloads,
            "asset_transport": transfer::sample(&holding)?,
        }))?
    );
    Ok(())
}
