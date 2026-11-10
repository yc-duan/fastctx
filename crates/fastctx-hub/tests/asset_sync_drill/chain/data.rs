use super::KIND;
use super::*;
use fastctx_sync::{
    catalog::*,
    cursor::*,
    identity::*,
    manifest::*,
    registry::REGISTRY,
    store::{access::*, ingest::*, ingest_effects::SourceEffects, ingest_memory::IngestBudget, sources::SqliteSources},
};
use fastctx_types::sync::{conversation_identity::*, conversation_timeline::*, metadata::MetadataCodec};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(super) struct Data {
    pub records: Vec<SourceRecord>,
    pub blocks: Vec<ContentBlock>,
    pub bytes: u64,
}
pub(super) fn target(c: &Config) -> SourceTarget {
    let object = c.object();
    let epoch = ProducerEpoch::from_bytes([42; 32]);
    SourceTarget {
        source: SourceId::from_bytes([42; 32]),
        stream: StreamId::of(&object, KIND),
        epoch: epoch.clone(),
        producer: Producer::Device { device: c.device(0) },
        custody: Custody {
            context: node::context(c, 0),
            kind: REGISTRY.asset(KIND).unwrap().registered(),
            object,
            holding: node::holding(c, 0),
            purpose: HoldingUse::RuntimeSource {
                host: HostBinding {
                    conversation: c.conversation(),
                    account: c.account.id.clone(),
                    host: c.device(0),
                    epoch,
                },
            },
        },
    }
}
pub(super) fn sources(c: &Config, blocks: &fastctx_sync::blocks::SqliteBlocks) -> anyhow::Result<SqliteSources> {
    Ok(SqliteSources::new(
        blocks.clone(),
        Arc::new(fastctx_net::sync_proof::IdentitySigner::device(
            c.hub.clone(),
            Arc::new(c.key(0)),
            Arc::new(fastctx_sync::metrics::PlatformMetrics),
        )),
        vec![Arc::new(policy::Gate(c.conversation()))],
        IngestBudget::for_machine(None)?,
    )?
    .with_hub(c.identity.clone(), c.verifier())?)
}
pub(super) fn key(c: &Config) -> fastctx_sync::replicate::SourceUploadKey {
    let t = target(c);
    fastctx_sync::replicate::SourceUploadKey { scope: t.custody.object.scope, stream: t.stream, epoch: t.epoch }
}
async fn block(
    s: &SqliteSources,
    bound: &fastctx_sync::store::source_step::SourceStepTarget,
    bytes: Vec<u8>,
) -> anyhow::Result<ContentBlock> {
    let encoded =
        fastctx_sync::blocks::codec::encode(bound.target().custody.object.scope.clone(), bytes, Encoding::Raw)?;
    let content = encoded.representation.content.clone();
    s.put_block(bound.clone(), encoded.representation, encoded.bytes).await?;
    Ok(content)
}
async fn page(
    s: &SqliteSources,
    bound: &fastctx_sync::store::source_step::SourceStepTarget,
    page: ManifestPage,
) -> anyhow::Result<ManifestRoot> {
    let page_json = serde_json::to_string(&page)?;
    let root = ManifestRoot(BlobId {
        domain: bound.target().custody.object.scope.clone(),
        algorithm: DigestAlgorithm::Sha256,
        digest: Digest::of(page_json.as_bytes()),
    });
    s.put_manifest(bound.clone(), ManifestReply { root: root.clone(), page_json }).await?;
    Ok(root)
}
pub(super) async fn prepare(root: &Path, c: &Config, per_record: u32) -> anyhow::Result<()> {
    let (_db, blocks) = node::blocks(&root.join("node-0"), c, 0)?;
    let s = sources(c, &blocks)?;
    let t = target(c);
    let step = s.begin_step(t.clone(), None, HostGeneration::from_bytes([42; 32])).await?;
    let bound = s.bind_target(step.clone(), t.clone()).await?;
    let display = ItemDisplayV1 {
        version: 1,
        kind: DisplayKind::User,
        facts: None,
        tool: None,
        status: None,
        append: None,
        parts: Vec::new(),
        more: None,
    };
    let payload = block(&s, &bound, display.encode().map_err(anyhow::Error::msg)?).await?;
    let display =
        page(&s, &bound, ManifestPage::Metadata { codec: MetadataCodec::DisplayV1, payload, references: Vec::new() })
            .await?;
    let mut records = Vec::new();
    let mut all = Vec::new();
    for n in 0..4_u64 {
        let mut children = Vec::new();
        for i in 0..per_record {
            let mut bytes = vec![(n + 1) as u8; fastctx_types::sync::BLOCK_BYTES as usize];
            bytes[..8].copy_from_slice(&(n * u64::from(per_record) + u64::from(i)).to_be_bytes());
            let content = block(&s, &bound, bytes).await?;
            all.push(content.clone());
            children.push(content);
        }
        let original = ContentRef::Manifest {
            root: page(&s, &bound, ManifestPage::Blocks { offset: ByteCount(0), blocks: children }).await?,
        };
        let sequence = Sequence(n + 1);
        let op = OperationId::from_bytes(Digest::of(format!("chain/{n}").as_bytes()).bytes());
        let event = TimelineEvent {
            evidence: Evidence {
                record: RecordRef { stream: t.stream.clone(), epoch: t.epoch.clone(), sequence },
                operation: op.clone(),
                accepted: sequence,
            },
            fact: TimelineFact::Observed {
                observation: Observation {
                    identity: ItemIdentity::Native(ItemKey {
                        conversation: c.conversation().object,
                        role: ItemRole::User,
                        native: NativeItemKey::ClaudeMessage { uuid: NativeId::new(format!("private-drill-{n}"))? },
                    }),
                    created: sequence,
                    authority: BodyAuthority::HostFinal,
                    placement: None,
                    body: BodySnapshot {
                        original,
                        display: ContentRef::Manifest { root: display.clone() },
                        complete: true,
                        text_part: None,
                        text_bytes: ByteCount(0),
                    },
                },
            },
        };
        let roots = fastctx_sync::conversation::source::direct_roots(&t.custody.object.scope, &event.fact)
            .map_err(anyhow::Error::msg)?;
        let payload =
            block(&s, &bound, fastctx_sync::conversation::source::event::encode(&event).map_err(anyhow::Error::msg)?)
                .await?;
        records.push(SourceRecord {
            sequence,
            operation: op,
            kind: KIND.into(),
            schema: 1,
            payload,
            roots,
            causes: Vec::new(),
        });
    }
    let batch = SourceBatch {
        format: 1,
        scope: t.custody.object.scope.clone(),
        stream: t.stream.clone(),
        producer: t.producer.clone(),
        epoch: t.epoch.clone(),
        operation: step.operation().clone(),
        kind: KIND.into(),
        schema: 1,
        after: Sequence(0),
        through: Sequence(4),
        records: records.clone(),
    };
    let next = SourceCheckpoint {
        source: t.source.clone(),
        generation: step.generation().clone(),
        revision: step.revision(),
        committed_offset: ByteCount(4),
        parser_version: 1,
        state: None,
    };
    s.ingest(IngestOperation::new(
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
    let data = Data { records, bytes: all.len() as u64 * u64::from(fastctx_types::sync::BLOCK_BYTES), blocks: all };
    std::fs::write(root.join("data.json"), serde_json::to_vec(&data)?)?;
    Ok(())
}
pub(super) fn load(root: &Path) -> anyhow::Result<Data> {
    Ok(serde_json::from_slice(&std::fs::read(root.join("data.json"))?)?)
}
