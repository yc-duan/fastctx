//! Staging probes and explicit interpretation of an already supplied page.
use fastctx_types::sync::{
    catalog::SourceBatch,
    cursor::HoldingPeriod,
    identity::{Digest, ObjectId, ObjectKey},
    manifest::{BlobId, ContentBlock, ContentRef, DigestAlgorithm, ManifestPage, ManifestRoot},
    source_transfer::SourceTransfer,
    transfer::{KindVersion, Message},
};

pub fn sample(holding: &HoldingPeriod, batch: &SourceBatch) -> Result<Vec<Message>, Box<dyn std::error::Error>> {
    let page = serde_json::to_vec(&ManifestPage::Directory { entries: Vec::new() })?;
    let block = ContentBlock {
        id: BlobId { domain: holding.scope.clone(), algorithm: DigestAlgorithm::Sha256, digest: Digest::of(&page) },
        decoded_bytes: page.len().try_into()?,
    };
    let upload = SourceTransfer {
        holding: holding.clone(),
        object: ObjectKey {
            scope: holding.scope.clone(),
            object: fastctx_types::sync::conversation_identity::source_object(
                &ObjectId::from_bytes([42; 32]),
                &batch.kind,
            )
            .ok_or("sample kind has no source object")?,
        },
        kind: KindVersion { kind: batch.kind.clone(), schema: batch.schema },
        epoch: batch.epoch.clone(),
        operation: batch.operation.clone(),
    };
    let raw = ContentRef::from(block.clone());
    let manifest = ContentRef::Manifest { root: ManifestRoot(block.id.clone()) };
    Ok(vec![
        Message::ProbeSource { upload: upload.clone(), content: raw.clone() },
        Message::SourcePresence { upload: upload.clone(), content: raw, present: false },
        Message::PublishSourceManifest { upload: upload.clone(), page: block },
        Message::ProbeSource { upload: upload.clone(), content: manifest.clone() },
        Message::SourcePresence { upload, content: manifest, present: true },
    ])
}
