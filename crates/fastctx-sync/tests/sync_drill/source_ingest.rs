//! A step commit lands its checkpoint, facts, roots, effects and closure together;
//! a retry returns the same receipt and a refusal leaves everything as it was.
use super::{source_steps::target, *};
use fastctx_sync::{
    catalog::{SourceBatch, SourceRecord},
    manifest::{ContentBlock, ContentRef},
    proof::{
        CommittedFragment, ConfirmedEpoch, EpochDescriptor, HubKeyRotation, ProducerRelease, ProofSigner,
        SignedAcceptanceFragment, SignedSourceBatch, SignedStatement,
    },
    store::{
        Error,
        ingest::{
            IngestBatch, IngestFacts, IngestOperation, IngestStatus, SourceCheckpoint, SourceIngest, SourceTarget,
        },
        ingest::{StateRead, StateReply, StreamProgress},
        ingest_effects::{DirtyProjection, IndexValue, SourceEffects, SourceFamily, SourceIndexChange},
        ingest_memory::{IngestBudget, MAX_SOURCE_BYTES, SourceAdmission},
        mutation::Expected,
        source_authorize::{PreparedSourceChange, SourceAuthorizer},
        source_step::{SourceStep, SourceStepTarget},
        sources::{SqliteSources, StreamOpening},
    },
};
use fastctx_types::sync::{
    conversation_identity::{ImageSelector, ItemKey, ItemRole, NativeId, NativeItemKey},
    conversation_timeline::{
        BodyAuthority, BodySnapshot, DisplayChannel, DisplayKind, DisplayPart, DisplayPicture, ItemDisplayV1,
        ItemIdentity, PictureTarget, ProjectedBody,
    },
    cursor::{Count, Sequence},
    metadata::MetadataCodec,
    proof::{Bytes, ProofPurpose, Signature, SigningKey},
};

/// Signs nothing for real: the local writer stores whatever its signer returns.
pub(super) struct Signer;
impl ProofSigner for Signer {
    fn accepted(&self, _: &CommittedFragment) -> std::result::Result<SignedAcceptanceFragment, Error> {
        Err(Error::Unavailable("drill signer".into()))
    }
    fn source(&self, value: &SourceBatch) -> std::result::Result<SignedSourceBatch, Error> {
        Ok(SignedSourceBatch {
            proof: SignedStatement {
                purpose: ProofPurpose::SourceBatchV1,
                signer: SigningKey([3; 32]),
                data: Bytes(serde_json::to_vec(value).map_err(|e| Error::Invalid(e.to_string()))?),
                signature: Signature([7; 64]),
            },
        })
    }
    fn epoch(&self, _: &EpochDescriptor) -> std::result::Result<ConfirmedEpoch, Error> {
        Err(Error::Unavailable("drill signer".into()))
    }
    fn release(&self, _: &ProducerRelease) -> std::result::Result<SignedStatement, Error> {
        Err(Error::Unavailable("drill signer".into()))
    }
    fn exposure(
        &self,
        _: &fastctx_sync::exposure::ExposureReceipt,
    ) -> std::result::Result<fastctx_sync::exposure::SignedExposureReceipt, Error> {
        Err(Error::Unavailable("drill signer".into()))
    }
    fn rotation(&self, _: &HubKeyRotation) -> std::result::Result<SignedStatement, Error> {
        Err(Error::Unavailable("drill signer".into()))
    }
}

/// Allows a batch unless a payload says "deny"; counts the payloads it judged.
pub(super) struct Gate(pub(super) AtomicUsize);
impl SourceAuthorizer for Gate {
    fn kind(&self) -> &'static str {
        "conversation.timeline"
    }
    fn authorize(&self, change: PreparedSourceChange<'_>) -> Decision {
        self.0.fetch_add(change.payloads.len(), Ordering::AcqRel);
        if change.payloads.iter().any(|payload| payload.bytes == b"deny") {
            Decision::Denied { revision: Revision(1), reason: "the drill denies this payload".into() }
        } else {
            Decision::Allowed { revision: Revision(1) }
        }
    }
}

/// Runs content work to quiescence. Projections have no worker in this drill, so
/// their tasks are completed when seen; the count says how many there were.
async fn settle(db: &Database, blocks: &SqliteBlocks) -> Result<u32> {
    let scheduler = SqliteScheduler::new(db.clone(), operation(221), holder(), Duration::from_secs(10))?;
    let mut projections = 0;
    for _ in 0..256 {
        let leases = scheduler.due(now_millis(), 16).await?;
        if leases.is_empty() {
            return Ok(projections);
        }
        for lease in leases {
            let outcome = if lease.task.key.kind == "sync.projection" {
                projections += 1;
                fastctx_sync::scheduler::Outcome::Completed { through: lease.task.wanted }
            } else {
                blocks.process(lease.clone()).await?
            };
            scheduler.finish(lease, outcome).await?;
        }
    }
    Err("source work did not settle".into())
}
pub(super) fn count(db: &Database, sql: &str) -> Result<u32> {
    Ok(db.read_now(|db| Ok(db.query_row(sql, [], |row| row.get(0))?))?)
}
pub(super) async fn block(sources: &SqliteSources, target: &SourceStepTarget, bytes: &[u8]) -> Result<ContentBlock> {
    let content = codec::encode(scope(), bytes.to_vec(), Encoding::Raw)?;
    let block = content.representation.content.clone();
    sources.put_block(target.clone(), content.representation, content.bytes).await?;
    Ok(block)
}
pub(super) fn metadata(
    codec: MetadataCodec,
    payload: ContentBlock,
    references: Vec<ContentRef>,
) -> Result<ManifestReply> {
    let page_json = serde_json::to_string(&ManifestPage::Metadata { codec, payload, references })?;
    Ok(ManifestReply {
        root: ManifestRoot(BlobId {
            domain: scope(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(page_json.as_bytes()),
        }),
        page_json,
    })
}
/// A user message with one owned image, published as its DisplayV1 page.
pub(super) async fn display(
    sources: &SqliteSources,
    target: &SourceStepTarget,
    image: &[u8],
) -> Result<(ContentRef, ContentRef, ContentBlock, ItemDisplayV1)> {
    let part = ContentRef::from(block(sources, target, image).await?);
    let descriptor = ItemDisplayV1 {
        version: 1,
        kind: DisplayKind::User,
        tool: None,
        status: None,
        facts: None,
        append: None,
        parts: vec![DisplayPart {
            index: Count(0),
            channel: DisplayChannel::Image,
            content: part.clone(),
            bytes: ByteCount(image.len() as u64),
            preview: None,
            media_type: Some("image/png".into()),
            picture: Some(DisplayPicture {
                selector: ImageSelector::ClaudeMessage { uuid: NativeId::new("u-1")?, image: Count(0) },
                target: PictureTarget::Owned { content: part.clone() },
            }),
        }],
        more: None,
    };
    let payload = block(sources, target, &descriptor.encode()?).await?;
    let page = metadata(MetadataCodec::DisplayV1, payload.clone(), vec![part.clone()])?;
    let root = ContentRef::Manifest { root: page.root.clone() };
    sources.put_manifest(target.clone(), page).await?;
    Ok((root, part, payload, descriptor))
}
/// The projected body of that message, published as its BodyV1 page.
pub(super) async fn body(
    sources: &SqliteSources,
    target: &SourceStepTarget,
    original: ContentRef,
    display: ContentRef,
) -> Result<ContentRef> {
    let projected = ProjectedBody {
        identity: ItemIdentity::Native(ItemKey {
            conversation: target.target().custody.object.object.clone(),
            role: ItemRole::User,
            native: NativeItemKey::ClaudeMessage { uuid: NativeId::new("u-1")? },
        }),
        primary: BodySnapshot { original, display, complete: true, text_part: None, text_bytes: ByteCount(0) },
        authority: BodyAuthority::HostFinal,
        world: None,
        host: None,
        run: None,
    };
    let payload = block(sources, target, &serde_json::to_vec(&projected)?).await?;
    let references = projected.owned_content().into_iter().cloned().collect();
    let page = metadata(MetadataCodec::BodyV1, payload, references)?;
    let root = ContentRef::Manifest { root: page.root.clone() };
    sources.put_manifest(target.clone(), page).await?;
    Ok(root)
}
fn batch(
    target: &SourceTarget,
    step: &SourceStep,
    after: u64,
    payload: ContentBlock,
    roots: Vec<ContentRef>,
) -> SourceBatch {
    let kind = REGISTRY.asset("conversation.timeline").unwrap();
    SourceBatch {
        format: 1,
        scope: scope(),
        stream: target.stream.clone(),
        producer: target.producer.clone(),
        epoch: target.epoch.clone(),
        operation: step.operation().clone(),
        kind: kind.registration().id.into(),
        schema: kind.registration().schema,
        after: Sequence(after),
        through: Sequence(after + 1),
        records: vec![SourceRecord {
            sequence: Sequence(after + 1),
            operation: OperationId::from_bytes([after as u8 + 100; 32]),
            kind: kind.registration().id.into(),
            schema: kind.registration().schema,
            payload,
            roots,
            causes: Vec::new(),
        }],
    }
}
pub(super) fn checkpoint(
    target: &SourceTarget,
    step: &SourceStep,
    offset: u64,
    state: Option<ContentRef>,
) -> SourceCheckpoint {
    SourceCheckpoint {
        source: target.source.clone(),
        generation: step.generation().clone(),
        revision: step.revision(),
        committed_offset: ByteCount(offset),
        parser_version: 1,
        state,
    }
}
fn effects(target: &SourceTarget, expected: Expected, value: &[u8]) -> Result<SourceEffects> {
    let derived = REGISTRY.projection("derived.timeline")?;
    Ok(SourceEffects::new(
        vec![SourceIndexChange {
            family: SourceFamily::HostRows,
            key: b"row-1".to_vec(),
            expected,
            value: IndexValue::Put(value.to_vec()),
        }],
        vec![DirtyProjection { kind: derived, object: target.custody.object.clone() }],
        Vec::new(),
    )?)
}

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("source-ingest");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(90), 1 << 23)?;
    let gate = Arc::new(Gate(AtomicUsize::new(0)));
    let memory = IngestBudget::for_machine(None)?;
    let sources = SqliteSources::new(
        blocks.clone(),
        Arc::new(Signer),
        vec![gate.clone() as Arc<dyn SourceAuthorizer>],
        memory.clone(),
    )?;
    let primary = target(&custody(90, "conversation.timeline"), 90);
    let generation = HostGeneration::from_bytes([90; 32]);
    // A kind without an installed authorizer cannot even open a step.
    let other = target(&custody(91, "conversation.artifact"), 91);
    assert!(matches!(sources.begin_step(other, None, generation.clone()).await, Err(Error::Unavailable(_))));

    // First step: a record whose display page owns an image, a parser state root,
    // an index row and a stale projection, all committed at once.
    let step = sources.begin_step(primary.clone(), None, generation.clone()).await?;
    let source_memory = sources.memory(&step)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let SourceAdmission::Granted(window) = source_memory.acquire(MAX_SOURCE_BYTES, deadline).await? else {
        return Err("source window was not granted".into());
    };
    assert_eq!(window.flush_threshold(), 4 * 1024 * 1024);
    drop(source_memory);
    assert!(matches!(sources.memory(&step)?.acquire(1, deadline).await?, SourceAdmission::ReleaseRequired));
    let uninstalled = SqliteSources::new(blocks.clone(), Arc::new(Signer), Vec::new(), memory.clone())?;
    assert!(matches!(uninstalled.memory(&step), Err(Error::Unavailable(_))));
    drop(window);
    let SourceAdmission::Granted(window) = sources.memory(&step)?.acquire(1, deadline).await? else {
        return Err("source window was not released".into());
    };
    assert_eq!((window.bytes(), window.flush_threshold()), (1, 1));
    drop((window, uninstalled));
    let bound = sources.bind_target(step.clone(), primary.clone()).await?;
    let (display, part, descriptor, _) = display(&sources, &bound, b"\x89PNG one image").await?;
    let original = ContentRef::from(block(&sources, &bound, br#"{"type":"user","uuid":"u-1"}"#).await?);
    let body = body(&sources, &bound, original, display.clone()).await?;
    let payload = block(&sources, &bound, br#"{"item":"u-1"}"#).await?;
    let state = ContentRef::from(block(&sources, &bound, b"parser state one").await?);
    // An open step's uploads are private to it, even to its own source handle.
    assert!(sources.read_block(primary.clone(), descriptor.clone()).await.is_err());
    let facts = batch(&primary, &step, 0, payload, vec![display.clone(), body.clone()]);
    let commit = IngestOperation::new(
        step.clone(),
        None,
        checkpoint(&primary, &step, 100, Some(state.clone())),
        vec![IngestBatch {
            target: bound.clone(),
            facts: Some(IngestFacts::Local { batch: facts }),
            effects: effects(&primary, Expected::Absent, b"first")?,
        }],
    )?;
    let receipt = sources.ingest(commit.clone()).await?;
    assert_eq!(gate.0.load(Ordering::Acquire), 1);
    assert!(matches!(
        &receipt.sources[0].status,
        IngestStatus::Applied { progress: StreamProgress::LocalPending { cursor } } if cursor.committed == Sequence(1)
    ));
    assert_eq!(receipt.leases.len(), 1, "the checkpoint state is held");
    // A producer recognizes an exact replay by the record its operation holds.
    let recorded =
        sources.recorded(primary.clone(), OperationId::from_bytes([100; 32])).await?.ok_or("record missing")?;
    assert_eq!(recorded.record.sequence, Sequence(1));
    assert!(sources.recorded(primary.clone(), OperationId::from_bytes([99; 32])).await?.is_none());
    // A lost reply is answered from the consumed step, without a second commit.
    let again = sources.ingest(commit).await?;
    assert_eq!((again.local, again.sources.len()), (receipt.local, 1));
    assert_eq!(gate.0.load(Ordering::Acquire), 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_stream_records")?, 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_stream_roots")?, 3);
    // A committed descriptor's own payload is readable at once, before any
    // content worker pass.
    let bytes = sources.read_block(primary.clone(), descriptor).await?;
    assert_eq!(ItemDisplayV1::decode(&bytes)?.parts.len(), 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_source_proofs WHERE state='local'")?, 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_stream_heads WHERE epoch_proof IS NULL")?, 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_source_steps WHERE state='consumed'")?, 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_asset_uploads WHERE consumed=1")?, 1);
    let found = sources.checkpoint(primary.clone()).await?.ok_or("checkpoint missing")?;
    assert_eq!(
        (found.revision, found.committed_offset, &found.state),
        (step.revision(), ByteCount(100), &Some(state.clone()))
    );
    assert!(matches!(sources.progress(primary.clone()).await?, Some(StreamProgress::LocalPending { .. })));
    let row =
        sources.get_index(primary.clone(), SourceFamily::HostRows, b"row-1".to_vec()).await?.ok_or("row missing")?;
    assert_eq!((row.revision, row.value.as_slice()), (Revision(1), b"first".as_slice()));
    // The record's display root is published to the object; expansion through
    // its validated page then grants the owned image as well.
    assert_eq!(settle(&db, &blocks).await?, 1, "the commit marked its projection stale");
    let granted = |hash: &str| {
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM sync_asset_access WHERE object='{}' AND hash='{hash}'",
                primary.custody.object.object.as_str()
            ),
        )
    };
    assert_eq!(granted(display.id().digest.as_str())?, 1);
    assert_eq!(granted(part.id().digest.as_str())?, 1);
    // The checkpoint's private state reads back only at its own revision.
    let state_id = state.id().clone();
    let read = sources
        .read_state(primary.clone(), StateRead::Block { checkpoint: step.revision(), id: state_id.clone() })
        .await?;
    assert!(matches!(read, StateReply::Bytes(bytes) if bytes == b"parser state one"));

    // Second step: checkpoint only, with a new state root that replaces the hold.
    let second = sources.begin_step(primary.clone(), Some(step.revision()), generation.clone()).await?;
    let bound = sources.bind_target(second.clone(), primary.clone()).await?;
    let state_two = ContentRef::from(block(&sources, &bound, b"parser state two").await?);
    let commit = IngestOperation::new(
        second.clone(),
        Some(found.clone()),
        checkpoint(&primary, &second, 140, Some(state_two.clone())),
        vec![IngestBatch { target: bound.clone(), facts: None, effects: SourceEffects::default() }],
    )?;
    let receipt = sources.ingest(commit).await?;
    assert!(matches!(receipt.sources[0].status, IngestStatus::CheckpointOnly));
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM sync_root_leases WHERE released=1 AND supply_id IS NULL")?,
        1,
        "the old state hold is released"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM sync_root_leases WHERE released=0 AND supply_id IS NOT NULL")?,
        0,
        "completed supply checks release their private protection"
    );
    let stale =
        sources.read_state(primary.clone(), StateRead::Block { checkpoint: step.revision(), id: state_id }).await?;
    assert!(matches!(stale, StateReply::Superseded { current: Some(current) } if current == second.revision()));
    let found = sources.checkpoint(primary.clone()).await?.ok_or("checkpoint missing")?;

    // Refusals change nothing: a stale stream start, content the step never
    // supplied and a denied payload each leave the step active and the head put.
    let third = sources.begin_step(primary.clone(), Some(second.revision()), generation.clone()).await?;
    let bound = sources.bind_target(third.clone(), primary.clone()).await?;
    let payload = block(&sources, &bound, br#"{"item":"u-2"}"#).await?;
    let attempt = |after: u64, payload: ContentBlock, roots: Vec<ContentRef>, expected: Expected| {
        IngestOperation::new(
            third.clone(),
            Some(found.clone()),
            checkpoint(&primary, &third, 180, Some(state_two.clone())),
            vec![IngestBatch {
                target: bound.clone(),
                facts: Some(IngestFacts::Local { batch: batch(&primary, &third, after, payload, roots) }),
                effects: effects(&primary, expected, b"second").unwrap(),
            }],
        )
    };
    let refused = |result: std::result::Result<_, Error>, why: &str| {
        assert!(matches!(&result, Err(Error::Invalid(reason)) if reason.contains(why)), "{why}: {:?}", result.err());
    };
    refused(
        sources.ingest(attempt(0, payload.clone(), Vec::new(), Expected::Revision(Revision(1)))?).await,
        "stream head is at 1",
    );
    let foreign = ContentRef::Single {
        block: BlobId { domain: scope(), algorithm: DigestAlgorithm::Sha256, digest: Digest::of(b"never supplied") },
        decoded_bytes: 14,
    };
    refused(
        sources.ingest(attempt(1, payload.clone(), vec![foreign], Expected::Revision(Revision(1)))?).await,
        "has no qualified supply",
    );
    refused(sources.ingest(attempt(1, payload.clone(), Vec::new(), Expected::Absent)?).await, "index row moved");
    let denied = block(&sources, &bound, b"deny").await?;
    assert!(matches!(
        sources.ingest(attempt(1, denied, Vec::new(), Expected::Revision(Revision(1)))?).await,
        Err(Error::Denied(_))
    ));
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_stream_records")?, 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_source_steps WHERE state='active'")?, 1);
    // A tree adopted through one root is readable to its leaves as soon as the
    // receipt returns, before any content worker pass.
    let (shown, image, _, _) = self::display(&sources, &bound, b"\x89PNG deep").await?;
    let original = ContentRef::from(block(&sources, &bound, br#"{"uuid":"u-2"}"#).await?);
    let deep = self::body(&sources, &bound, original, shown).await?;
    let receipt = sources.ingest(attempt(1, payload, vec![deep], Expected::Revision(Revision(1)))?).await?;
    assert!(matches!(
        &receipt.sources[0].status,
        IngestStatus::Applied { progress: StreamProgress::LocalPending { cursor } } if cursor.committed == Sequence(2)
    ));
    assert_eq!(granted(image.id().digest.as_str())?, 1);
    let row =
        sources.get_index(primary.clone(), SourceFamily::HostRows, b"row-1".to_vec()).await?.ok_or("row missing")?;
    assert_eq!(row.revision, Revision(2));

    // A producer's stream epoch is chosen once: the first candidate persists
    // with an empty head, and later openings, across a restart too, return it.
    let opening = |candidate: u8, device: u8| StreamOpening {
        object: custody(92, "conversation.timeline").object,
        kind: REGISTRY.asset("conversation.timeline").unwrap().registered(),
        producer: Producer::Device { device: DeviceId::from_bytes([device; 32]) },
        candidate: ProducerEpoch::from_bytes([candidate; 32]),
    };
    let epoch = sources.open_stream(opening(1, 3)).await?;
    assert_eq!(epoch, ProducerEpoch::from_bytes([1; 32]));
    assert!(matches!(sources.open_stream(opening(2, 4)).await, Err(Error::Invalid(_))));

    // Committed steps drain their staged pins; a restart keeps everything.
    while blocks.drain_steps(64).await? != 0 {}
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_asset_pins WHERE phase!='released'")?, 0);
    drop((sources, blocks));
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let db = open(&path)?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(90), 1 << 23)?;
    let sources = SqliteSources::new(blocks, Arc::new(Signer), vec![gate as Arc<dyn SourceAuthorizer>], memory)?;
    let found = sources.checkpoint(primary.clone()).await?.ok_or("checkpoint missing")?;
    assert_eq!(found.revision, third.revision());
    assert_eq!(sources.open_stream(opening(2, 3)).await?, epoch);
    println!(
        "source ingest: atomic step commit, retried receipt, owned display parts, checkpoint state handover, index and projection effects, refusals without writes, stream epoch opening and restart passed"
    );
    Ok(())
}
