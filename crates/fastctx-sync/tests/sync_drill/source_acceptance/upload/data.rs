//! Actual source-step publication includes a shared nested manifest and split proofs.
use super::*;
use fastctx_sync::{
    catalog::RecordRef,
    manifest::{
        BlobId, ContentBlock, DigestAlgorithm, DirectoryEntry, ManifestPage, ManifestReply, ManifestRoot, PathName,
        RelativePath,
    },
};
use fastctx_types::sync::{
    conversation_timeline::{DisplayKind, ItemDisplayV1},
    metadata::MetadataCodec,
};

async fn block(
    sources: &SqliteSources,
    bound: &fastctx_sync::store::source_step::SourceStepTarget,
    bytes: Vec<u8>,
) -> Result<ContentBlock> {
    let encoded = codec::encode(bound.target().custody.object.scope.clone(), bytes, Encoding::Raw)?;
    let content = encoded.representation.content.clone();
    sources.put_block(bound.clone(), encoded.representation, encoded.bytes).await?;
    Ok(content)
}
fn page(scope: &ScopeKey, value: ManifestPage) -> Result<ManifestReply> {
    let page_json = serde_json::to_string(&value)?;
    Ok(ManifestReply {
        root: ManifestRoot(BlobId {
            domain: scope.clone(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(page_json.as_bytes()),
        }),
        page_json,
    })
}
pub(super) async fn ingest(sources: &SqliteSources, target: &SourceTarget) -> Result<SourceUploadKey> {
    let step = sources.begin_step(target.clone(), None, HostGeneration::from_bytes([42; 32])).await?;
    let bound = sources.bind_target(step.clone(), target.clone()).await?;
    let descriptor = ItemDisplayV1 {
        version: 1,
        kind: DisplayKind::User,
        tool: None,
        status: None,
        append: None,
        parts: Vec::new(),
        more: None,
        facts: None,
    };
    let payload = block(sources, &bound, descriptor.encode()?).await?;
    let display = page(
        &target.custody.object.scope,
        ManifestPage::Metadata { codec: MetadataCodec::DisplayV1, payload, references: Vec::new() },
    )?;
    sources.put_manifest(bound.clone(), display.clone()).await?;
    let root = page(
        &target.custody.object.scope,
        ManifestPage::Directory {
            entries: vec![
                DirectoryEntry::Directory {
                    path: RelativePath { components: vec![PathName::Utf8("display".into())] },
                    root: Some(display.root),
                },
                DirectoryEntry::Link {
                    path: RelativePath { components: vec![PathName::Utf8("padding".into())] },
                    target: PathName::Utf8("a".repeat(255000)),
                },
            ],
        },
    )?;
    assert!(root.page_json.len() > 250000);
    sources.put_manifest(bound.clone(), root.clone()).await?;
    commit(sources, target, step, bound, vec![root.root; 2]).await
}

pub(super) async fn bulk(
    sources: &SqliteSources,
    target: &SourceTarget,
    blocks_per_batch: u8,
) -> Result<SourceUploadKey> {
    let step = sources.begin_step(target.clone(), None, HostGeneration::from_bytes([42; 32])).await?;
    let bound = sources.bind_target(step.clone(), target.clone()).await?;
    let mut roots = Vec::new();
    for batch in 0..4_u8 {
        let mut blocks = Vec::new();
        for index in 0..blocks_per_batch {
            let mut bytes = vec![batch; fastctx_types::sync::BLOCK_BYTES as usize];
            bytes[0] = index;
            blocks.push(block(sources, &bound, bytes).await?);
        }
        let manifest = page(&target.custody.object.scope, ManifestPage::Blocks { offset: ByteCount(0), blocks })?;
        sources.put_manifest(bound.clone(), manifest.clone()).await?;
        roots.push(manifest.root);
    }
    commit(sources, target, step, bound, roots).await
}

async fn commit(
    sources: &SqliteSources,
    target: &SourceTarget,
    step: fastctx_sync::store::source_step::SourceStep,
    bound: fastctx_sync::store::source_step::SourceStepTarget,
    roots: Vec<ManifestRoot>,
) -> Result<SourceUploadKey> {
    let count = roots.len() as u64;
    let mut records = Vec::new();
    for (index, root) in roots.into_iter().enumerate() {
        let sequence = index as u64 + 1;
        let payload = block(sources, &bound, format!("source fact {sequence}").into_bytes()).await?;
        records.push(SourceRecord {
            sequence: Sequence(sequence),
            operation: operation(sequence as u8),
            kind: KIND.into(),
            schema: 1,
            payload,
            roots: vec![ContentRef::Manifest { root }],
            causes: (1..=350)
                .map(|sequence| RecordRef {
                    stream: StreamId::from_bytes([80; 32]),
                    epoch: ProducerEpoch::from_bytes([81; 32]),
                    sequence: Sequence(sequence),
                })
                .collect(),
        });
    }
    let batch = SourceBatch {
        format: 1,
        scope: target.custody.object.scope.clone(),
        stream: target.stream.clone(),
        producer: target.producer.clone(),
        epoch: target.epoch.clone(),
        operation: step.operation().clone(),
        kind: KIND.into(),
        schema: 1,
        after: Sequence(0),
        through: Sequence(count),
        records,
    };
    let next = SourceCheckpoint {
        source: target.source.clone(),
        generation: step.generation().clone(),
        revision: step.revision(),
        committed_offset: ByteCount(count),
        parser_version: 1,
        state: None,
    };
    sources
        .ingest(IngestOperation::new(
            step,
            None,
            next,
            vec![IngestBatch {
                target: bound,
                facts: Some(IngestFacts::Local { batch }),
                effects: SourceEffects::default(),
            }],
        )?)
        .await?;
    Ok(SourceUploadKey {
        scope: target.custody.object.scope.clone(),
        stream: target.stream.clone(),
        epoch: target.epoch.clone(),
    })
}
