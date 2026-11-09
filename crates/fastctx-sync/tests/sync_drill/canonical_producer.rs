//! Canonical production, restart and installation share the real source writer.
use super::{
    source_ingest::{Gate, Signer, checkpoint, count},
    source_steps::target,
    *,
};
use fastctx_sync::{
    catalog::RecordRef,
    conversation::{ItemState, producer::*},
    manifest::{ContentBlock, ContentRef},
    store::{
        ingest::{IngestBatch, IngestFacts, IngestOperation, SourceCheckpoint, SourceIngest, SourceTarget},
        ingest_effects::{DirtyProjection, SourceEffects, SourceFamily},
        source_authorize::SourceAuthorizer,
        source_step::SourceStepTarget,
        sources::SqliteSources,
        timelines::SqliteTimelines,
    },
};
use fastctx_types::sync::{
    conversation_identity::{ItemKey, ItemRole, NativeId, NativeItemKey, NativeTurnKey, TurnKey},
    conversation_timeline::*,
    cursor::{Count, Sequence},
    metadata::MetadataCodec,
};
use std::collections::BTreeMap;

#[derive(Default)]
struct Supplied {
    blocks: BTreeMap<BlobId, Vec<u8>>,
    pages: BTreeMap<ManifestRoot, ManifestReply>,
}
impl SuppliedContent for Supplied {
    fn block(&self, b: &ContentBlock) -> Option<&[u8]> {
        self.blocks.get(&b.id).map(Vec::as_slice)
    }
    fn manifest(&self, r: &ManifestRoot) -> Option<&ManifestReply> {
        self.pages.get(r)
    }
}
impl Supplied {
    async fn publish_block(&mut self, s: &SqliteSources, t: &SourceStepTarget, bytes: &[u8]) -> Result<ContentRef> {
        let encoded = codec::encode(scope(), bytes.to_vec(), Encoding::Raw)?;
        let block = encoded.representation.content.clone();
        s.put_block(t.clone(), encoded.representation, encoded.bytes).await?;
        self.blocks.insert(block.id.clone(), bytes.to_vec());
        Ok(block.into())
    }
    async fn display(&mut self, s: &SqliteSources, t: &SourceStepTarget, text: &str) -> Result<ContentRef> {
        let content = self.publish_block(s, t, text.as_bytes()).await?;
        let d = ItemDisplayV1 {
            version: 1,
            kind: DisplayKind::Message,
            tool: None,
            status: Some(DisplayStatus::Running),
            append: Some(DisplayAppend { channel: fastctx_types::session::Part::Text, index: Count(0) }),
            parts: vec![DisplayPart {
                index: Count(0),
                channel: DisplayChannel::Text,
                content,
                bytes: ByteCount(text.len() as u64),
                preview: Some(text.into()),
                media_type: None,
                picture: None,
            }],
            more: None,
        };
        self.metadata(s, t, MetadataCodec::DisplayV1, &d.encode()?, d.owned_content()).await
    }
    async fn metadata(
        &mut self,
        s: &SqliteSources,
        t: &SourceStepTarget,
        codec: MetadataCodec,
        bytes: &[u8],
        references: Vec<ContentRef>,
    ) -> Result<ContentRef> {
        let payload = self.publish_block(s, t, bytes).await?;
        let ContentRef::Single { block, decoded_bytes } = payload else { unreachable!() };
        let page_json = serde_json::to_string(&ManifestPage::Metadata {
            codec,
            payload: ContentBlock { id: block, decoded_bytes },
            references,
        })?;
        let root = ManifestRoot(BlobId {
            domain: scope(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(page_json.as_bytes()),
        });
        let page = ManifestReply { root: root.clone(), page_json };
        s.put_manifest(t.clone(), page.clone()).await?;
        self.pages.insert(root.clone(), page);
        Ok(ContentRef::Manifest { root })
    }
}
fn input(t: &SourceStepTarget, sequence: u64, accepted: u64, fact: TimelineFact) -> MergeInput {
    MergeInput {
        event: TimelineEvent {
            evidence: Evidence {
                record: RecordRef {
                    stream: t.target().stream.clone(),
                    epoch: t.target().epoch.clone(),
                    sequence: Sequence(sequence),
                },
                operation: OperationId::from_bytes(Digest::of(&accepted.to_be_bytes()).bytes()),
                accepted: Sequence(accepted),
            },
            fact,
        },
        causes: Vec::new(),
        agent_ancestors: Vec::new(),
    }
}
fn fingerprint(o: &MergeOutput) -> Result<Vec<u8>> {
    let facts = match &o.facts {
        Some(IngestFacts::Local { batch }) => serde_json::to_vec(batch)?,
        _ => Vec::new(),
    };
    Ok([facts, format!("{:?}", o.effects.indexes()).into_bytes()].concat())
}
async fn commit(
    s: &SqliteSources,
    t: &SourceStepTarget,
    expected: Option<SourceCheckpoint>,
    out: MergeOutput,
) -> Result<SourceCheckpoint> {
    let next = checkpoint(t.target(), t.step(), expected.as_ref().map_or(1, |p| p.committed_offset.0 + 1), None);
    s.ingest(IngestOperation::new(
        t.step().clone(),
        expected,
        next.clone(),
        vec![IngestBatch { target: t.clone(), facts: out.facts, effects: out.effects }],
    )?)
    .await?;
    Ok(next)
}
fn settle<'a>(
    db: &'a Database,
    b: &'a SqliteBlocks,
    t: &'a SqliteTimelines,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let scheduler = SqliteScheduler::new(db.clone(), operation(225), holder(), Duration::from_secs(10))?;
        for _ in 0..256 {
            let jobs = scheduler.due(now_millis(), 16).await?;
            if jobs.is_empty() {
                return Ok(());
            }
            for job in jobs {
                let outcome = if job.task.key.kind == "sync.projection" {
                    t.process(job.clone()).await?
                } else {
                    b.process(job.clone()).await?
                };
                scheduler.finish(job, outcome).await?;
            }
        }
        Err("canonical work did not settle".into())
    })
}
fn effects(t: &SourceTarget) -> std::result::Result<SourceEffects, fastctx_sync::store::Error> {
    SourceEffects::new(
        Vec::new(),
        vec![DirtyProjection {
            kind: REGISTRY.projection("derived.timeline").map_err(fastctx_sync::store::Error::Invalid)?,
            object: t.custody.object.clone(),
        }],
        Vec::new(),
    )
}

async fn acquire_window(
    s: &SqliteSources,
    t: &SourceStepTarget,
) -> Result<fastctx_sync::store::ingest_memory::SourceWindow> {
    let memory = s.memory(t.step())?;
    match memory.acquire(12 << 20, tokio::time::Instant::now() + Duration::from_secs(10)).await? {
        fastctx_sync::store::ingest_memory::SourceAdmission::Granted(window) => Ok(window),
        fastctx_sync::store::ingest_memory::SourceAdmission::ReleaseRequired => {
            Err("canonical step retained an earlier working window".into())
        }
    }
}

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("canonical-producer");
    fastctx_platform::ensure_private_dir(&dir)?;
    let db = open(&dir.join("replica.sqlite3"))?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(96), 1 << 24)?;
    let sources = SqliteSources::new(
        blocks.clone(),
        Arc::new(Signer),
        vec![Arc::new(Gate(AtomicUsize::new(0))) as Arc<dyn SourceAuthorizer>],
        fastctx_sync::store::ingest_memory::IngestBudget::new(32 << 20)?,
    )?;
    let timelines = SqliteTimelines::new(blocks.clone());
    let conversation = ObjectId::from_bytes([196; 32]);
    let mut owner = custody(96, "conversation.timeline");
    owner.object.object = fastctx_types::sync::conversation_identity::ConversationObject::Timeline.id(&conversation);
    let primary = target(&owner, 96);
    let generation = HostGeneration::from_bytes([96; 32]);
    let initial = InitialState {
        active_branch: ObjectId::from_bytes([220; 32]),
        sources: vec![SourceSeed { source: primary.source.clone(), generation: generation.clone() }],
    };
    let key = ItemKey {
        conversation: conversation.clone(),
        role: ItemRole::AssistantBlock,
        native: NativeItemKey::ClaudeMessage { uuid: NativeId::new("canonical-message")? },
    };
    let turn = TurnDeclaration {
        key: TurnKey {
            conversation: conversation.clone(),
            native: NativeTurnKey::Claude { session: NativeId::new("session")?, turn: NativeId::new("turn")? },
        },
        order: Sequence(1),
    };
    let first = sources.begin_step(primary.clone(), None, generation.clone()).await?;
    let bound = sources.bind_target(first.clone(), primary.clone()).await?;
    let window = acquire_window(&sources, &bound).await?;
    let mut cache = Supplied::default();
    let display = cache.display(&sources, &bound, "hello").await?;
    let original = cache.publish_block(&sources, &bound, b"original").await?;
    let observed = input(
        &bound,
        1,
        1,
        TimelineFact::Observed {
            observation: Observation {
                identity: ItemIdentity::Native(key.clone()),
                created: Sequence(1),
                authority: BodyAuthority::HostDraft,
                body: BodySnapshot {
                    original,
                    display,
                    complete: false,
                    text_part: Some(fastctx_types::session::Part::Text),
                    text_bytes: ByteCount(5),
                },
                placement: Some(Placement {
                    turn: turn.key.id(),
                    slot: DisplaySlot::Sibling,
                    proof: PlacementProof::NativeTurn,
                }),
            },
        },
    );
    let inputs = [observed.clone()];
    let prepare = || MergeRequest {
        target: &bound,
        conversation: &conversation,
        after: Sequence(0),
        inputs: &inputs,
        supplied: &cache,
        initial: Some(&initial),
        other_effects: effects(&primary).unwrap(),
        budget: MergeBudget::default(),
    };
    let one = plan_step(&sources, prepare()).await?.publish(&sources).await?;
    let two = plan_step(&sources, prepare()).await?.publish(&sources).await?;
    assert_eq!(fingerprint(&one)?, fingerprint(&two)?);
    drop(two);
    assert!(one.progress.waiting_dependencies);
    assert!(!one.progress.ready_work);
    let done = commit(&sources, &bound, None, one).await?;
    drop(cache);
    drop(window);
    settle(&db, &blocks, &timelines).await?;
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_timeline_items WHERE active=1")?, 0);
    let second = sources.begin_step(primary.clone(), Some(done.revision), generation.clone()).await?;
    let bound = sources.bind_target(second, primary.clone()).await?;
    let window = acquire_window(&sources, &bound).await?;
    let input_turn = [input(&bound, 2, 2, TimelineFact::Turn { turn })];
    let out = plan_step(
        &sources,
        MergeRequest {
            target: &bound,
            conversation: &conversation,
            after: Sequence(1),
            inputs: &input_turn,
            supplied: &(),
            initial: None,
            other_effects: effects(&primary)?,
            budget: MergeBudget::default(),
        },
    )
    .await?
    .publish(&sources)
    .await?;
    assert!(!out.progress.waiting_dependencies);
    assert!(!out.progress.ready_work);
    let done = commit(&sources, &bound, Some(done), out).await?;
    drop(window);
    settle(&db, &blocks, &timelines).await?;
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_timeline_items WHERE active=1")?, 1);
    let third = sources.begin_step(primary.clone(), Some(done.revision), generation.clone()).await?;
    let bound = sources.bind_target(third, primary.clone()).await?;
    let window = acquire_window(&sources, &bound).await?;
    let mut cache = Supplied::default();
    let chunk = cache.publish_block(&sources, &bound, b" world").await?;
    let append = [input(
        &bound,
        4,
        3,
        TimelineFact::TextAppend {
            append: TextAppend {
                item: key.id(),
                authority: BodyAuthority::HostDraft,
                previous: observed.event.evidence.operation.clone(),
                part: fastctx_types::session::Part::Text,
                offset: ByteCount(5),
                bytes: ByteCount(6),
                chunk,
            },
        },
    )];
    let request = || MergeRequest {
        target: &bound,
        conversation: &conversation,
        after: Sequence(3),
        inputs: &append,
        supplied: &cache,
        initial: None,
        other_effects: effects(&primary).unwrap(),
        budget: MergeBudget::default(),
    };
    let a = plan_step(&sources, request()).await?;
    let again = sources.begin_step(primary.clone(), bound.step().expected(), bound.step().generation().clone()).await?;
    assert_eq!(again.operation(), bound.step().operation());
    let b = plan_step(&sources, request()).await?;
    let out = a.publish(&sources).await?;
    let repeated = b.publish(&sources).await?;
    assert_eq!(fingerprint(&out)?, fingerprint(&repeated)?);
    drop(repeated);
    let done = commit(&sources, &bound, Some(done), out).await?;
    drop(cache);
    drop(window);
    settle(&db, &blocks, &timelines).await?;
    let row = sources
        .get_index(
            primary.clone(),
            SourceFamily::Reducer,
            format!("canon/v1/{}/item/{}", conversation.as_str(), key.id().as_str()).into_bytes(),
        )
        .await?
        .ok_or("missing item state")?;
    let state: ItemState = serde_json::from_slice(&row.value)?;
    assert_eq!(state.body().unwrap().primary.text_bytes, ByteCount(11));
    assert_eq!(state.revision, Revision(2));
    let installed: fastctx_sync::timeline::TimelineItem = serde_json::from_slice(&db.read_now(|d| {
        Ok(d.query_row("SELECT body FROM sync_timeline_items WHERE active=1", [], |r| r.get::<_, Vec<u8>>(0))?)
    })?)?;
    assert_eq!(installed.display.parts[0].preview.as_deref(), Some("hello world"));
    let fourth = sources.begin_step(primary.clone(), Some(done.revision), generation).await?;
    let bound = sources.bind_target(fourth, primary.clone()).await?;
    let window = acquire_window(&sources, &bound).await?;
    let replay = [observed];
    let out = plan_step(
        &sources,
        MergeRequest {
            target: &bound,
            conversation: &conversation,
            after: Sequence(5),
            inputs: &replay,
            supplied: &(),
            initial: None,
            other_effects: SourceEffects::default(),
            budget: MergeBudget::default(),
        },
    )
    .await?
    .publish(&sources)
    .await?;
    assert_eq!(out.progress.replayed_inputs, 1);
    assert!(out.facts.is_none());
    assert!(out.effects.indexes().is_empty());
    let mut conflicting = replay[0].clone();
    conflicting.event.evidence.accepted = Sequence(9999);
    assert!(matches!(
        plan_step(
            &sources,
            MergeRequest {
                target: &bound,
                conversation: &conversation,
                after: Sequence(5),
                inputs: &[conflicting],
                supplied: &(),
                initial: None,
                other_effects: SourceEffects::default(),
                budget: MergeBudget::default()
            }
        )
        .await,
        Err(MergeError::Reduction { error: fastctx_sync::conversation::Error::ConflictingReplay, .. })
    ));
    commit(&sources, &bound, Some(done), out).await?;
    drop(window);
    let before = state.clone();
    drop((sources, blocks, timelines));
    drop(db);
    super::super::scheduler::closed(&dir.join("replica.sqlite3")).await?;
    let reopened = open(&dir.join("replica.sqlite3"))?;
    let reopened_blocks =
        SqliteBlocks::new(reopened, dir.join("files"), holder(), policy(false), operation(96), 1 << 24)?;
    let reopened_sources = SqliteSources::new(
        reopened_blocks,
        Arc::new(Signer),
        vec![Arc::new(Gate(AtomicUsize::new(0))) as Arc<dyn SourceAuthorizer>],
        fastctx_sync::store::ingest_memory::IngestBudget::new(32 << 20)?,
    )?;
    let row = reopened_sources
        .get_index(
            primary.clone(),
            SourceFamily::Reducer,
            format!("canon/v1/{}/item/{}", conversation.as_str(), key.id().as_str()).into_bytes(),
        )
        .await?
        .ok_or("missing resumed state")?;
    assert_eq!(serde_json::from_slice::<ItemState>(&row.value)?, before);
    println!(
        "canonical producer: deterministic atomic input, durable dependency replay, text append, exact operation replay and real timeline installation passed"
    );
    Ok(())
}
