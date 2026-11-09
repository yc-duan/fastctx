//! Immutable index examples use only explicitly invalid signature placeholders.
use fastctx_sync::{blocks::codec, manifest::Encoding};
use fastctx_types::sync::{
    catalog::{RecordRef, SourceBatch},
    cursor::{HoldingPeriod, Revision, ScopeCursor, StreamCursor},
    identity::{Digest, ObjectId, ObjectKey},
    manifest::{BlobId, ContentRef, DigestAlgorithm, ManifestPage, ManifestReply, ManifestRoot},
    metadata::{
        CatalogLeaf, HistoryLeaf, HistoryState, IndexedTimelineItem, MetadataCodec, ProofReference, TimelineLeaf,
    },
    proof::{CommittedFragment, EpochLookup, SignedAcceptanceFragment, SignedSourceBatch},
    timeline::TimelineChange,
    transfer::Message,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn sample(
    holding: &HoldingPeriod,
    cursor: &ScopeCursor,
    accepted: &SignedAcceptanceFragment,
    binding: &CommittedFragment,
    source: &SignedSourceBatch,
    batch: &SourceBatch,
    timeline: &[TimelineChange],
) -> Result<(Vec<Message>, Vec<serde_json::Value>)> {
    let acceptance = codec::encode(holding.scope.clone(), serde_json::to_vec(accepted)?, Encoding::Raw)?;
    let evidence = ProofReference { id: accepted.proof.id(), payload: acceptance.representation.content };
    let object = &binding.fragment.objects[0];
    let history = HistoryLeaf {
        object: object.key.clone(),
        states: vec![HistoryState {
            version: object.allocated.clone(),
            proof: evidence.clone(),
            candidate: object.content.clone(),
            current_version: object.current_version.clone(),
            current_content: object.current_content.clone(),
        }],
        previous: None,
    };
    let Message::CatalogPage { page, .. } = super::catalog::sample(holding, cursor, accepted, binding, batch)? else {
        unreachable!()
    };
    let catalog = CatalogLeaf { cursor: cursor.clone(), objects: page.objects, proofs: vec![evidence.clone()] };
    let signed = codec::encode(holding.scope.clone(), serde_json::to_vec(source)?, Encoding::Raw)?;
    let source_ref = ProofReference { id: source.proof.id(), payload: signed.representation.content };
    let mut items = std::collections::BTreeMap::new();
    for (record, change) in batch.records.iter().zip(timeline) {
        if let TimelineChange::Put { item } = change {
            items.insert(
                item.id.clone(),
                IndexedTimelineItem {
                    item: item.clone(),
                    record: RecordRef {
                        stream: batch.stream.clone(),
                        epoch: batch.epoch.clone(),
                        sequence: record.sequence,
                    },
                    payload: record.payload.clone(),
                    batch: source_ref.id.clone(),
                },
            );
        }
    }
    let mut items: Vec<_> = items.into_values().collect();
    items.sort_by(|a, b| a.item.position.cmp(&b.item.position));
    let mut refs = vec![ContentRef::from(source_ref.payload.clone())];
    for item in &items {
        refs.push(ContentRef::from(item.payload.clone()));
        refs.push(item.item.body.clone());
    }
    let timeline = TimelineLeaf {
        conversation: ObjectKey { scope: holding.scope.clone(), object: ObjectId::from_bytes([50; 32]) },
        projection: Revision(1),
        through: StreamCursor {
            scope: holding.scope.clone(),
            stream: batch.stream.clone(),
            epoch: batch.epoch.clone(),
            committed: batch.through,
        },
        items,
        proofs: vec![source_ref],
    };
    let mut history_refs = vec![ContentRef::from(evidence.payload.clone())];
    history_refs.extend(object.content.clone());
    history_refs.extend(object.current_content.clone());
    let mut messages = Vec::new();
    let mut payloads = Vec::new();
    for (codec, payload, references) in [
        (MetadataCodec::HistoryV1, serde_json::to_value(history)?, history_refs),
        (MetadataCodec::CatalogV1, serde_json::to_value(catalog)?, vec![ContentRef::from(evidence.payload)]),
        (MetadataCodec::TimelineV1, serde_json::to_value(timeline)?, refs),
    ] {
        let encoded = codec::encode(holding.scope.clone(), serde_json::to_vec(&payload)?, Encoding::Raw)?;
        let page_json = serde_json::to_string(&ManifestPage::Metadata {
            codec,
            payload: encoded.representation.content,
            references,
        })?;
        let reply = ManifestReply {
            root: ManifestRoot(BlobId {
                domain: holding.scope.clone(),
                algorithm: DigestAlgorithm::Sha256,
                digest: Digest::of(page_json.as_bytes()),
            }),
            page_json,
        };
        messages.push(Message::Manifest { holding: holding.clone(), reply });
        payloads.push(payload);
    }
    messages.push(Message::GetEpoch {
        scope: holding.scope.clone(),
        lookup: EpochLookup::Proof { id: Digest::from_bytes([41; 32]) },
    });
    messages.push(Message::GetEpoch {
        scope: holding.scope.clone(),
        lookup: EpochLookup::Stream { stream: batch.stream.clone(), epoch: batch.epoch.clone() },
    });
    Ok((messages, payloads))
}
