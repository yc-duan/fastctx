//! Committed item revisions become an installed timeline: a new snapshot root,
//! position rows, watermarks readers resume from and retention by the head.
use super::{
    source_ingest::{Gate, Signer, block, body, checkpoint, count, display},
    source_steps::target,
    *,
};
use fastctx_sync::{
    catalog::{RecordRef, SourceBatch, SourceRecord},
    manifest::ContentRef,
    store::{
        ingest::{IngestBatch, IngestFacts, IngestOperation, SourceCheckpoint, SourceIngest, SourceTarget},
        ingest_effects::{DirtyProjection, SourceEffects},
        source_authorize::SourceAuthorizer,
        source_step::{SourceStep, SourceStepTarget},
        sources::SqliteSources,
        timelines::SqliteTimelines,
    },
    timeline::{TimelineChange, TimelineItem, TimelinePosition},
};
use fastctx_types::sync::{
    conversation_identity::{ItemKey, ItemRole, NativeId, NativeItemKey},
    conversation_timeline::{
        BodyAuthority, BodySnapshot, Evidence, ItemIdentity, Observation, TimelineEvent, TimelineFact,
    },
    cursor::Sequence,
};

/// One record's fact, with the roots it declares.
struct Fact {
    fact: TimelineFact,
    roots: Vec<ContentRef>,
}

fn identity(conversation: &ObjectId, uuid: &str) -> Result<ItemIdentity> {
    Ok(ItemIdentity::Native(ItemKey {
        conversation: conversation.clone(),
        role: ItemRole::User,
        native: NativeItemKey::ClaudeMessage { uuid: NativeId::new(uuid)? },
    }))
}
fn observed(identity: ItemIdentity, created: u64, original: ContentRef, display: ContentRef) -> Fact {
    let roots = vec![original.clone(), display.clone()];
    let body = BodySnapshot { original, display, complete: true, text_part: None, text_bytes: ByteCount(0) };
    Fact {
        fact: TimelineFact::Observed {
            observation: Observation {
                identity,
                created: Sequence(created),
                authority: BodyAuthority::HostFinal,
                body,
                placement: None,
            },
        },
        roots,
    }
}
fn put(item: TimelineItem, part: ContentRef) -> Fact {
    let roots = vec![item.body.clone(), part];
    Fact { fact: TimelineFact::ItemRevision { change: TimelineChange::Put { item } }, roots }
}

/// Commits one step whose single batch appends `facts` after `after`, and
/// marks the conversation's timeline stale.
async fn commit(
    sources: &SqliteSources,
    step: &SourceStep,
    bound: &SourceStepTarget,
    primary: &SourceTarget,
    expected: Option<SourceCheckpoint>,
    after: u64,
    facts: Vec<Fact>,
) -> Result<SourceCheckpoint> {
    let kind = REGISTRY.asset("conversation.timeline").unwrap();
    let mut records = Vec::new();
    for (index, fact) in facts.into_iter().enumerate() {
        let sequence = Sequence(after + index as u64 + 1);
        let operation = OperationId::from_bytes([after as u8 * 16 + index as u8 + 1; 32]);
        let event = TimelineEvent {
            evidence: Evidence {
                record: RecordRef { stream: primary.stream.clone(), epoch: primary.epoch.clone(), sequence },
                operation: operation.clone(),
                accepted: sequence,
            },
            fact: fact.fact,
        };
        let payload = block(sources, bound, &serde_json::to_vec(&event)?).await?;
        records.push(SourceRecord {
            sequence,
            operation,
            kind: kind.registration().id.into(),
            schema: kind.registration().schema,
            payload,
            roots: fact.roots,
            causes: Vec::new(),
        });
    }
    let batch = SourceBatch {
        format: 1,
        scope: scope(),
        stream: primary.stream.clone(),
        producer: primary.producer.clone(),
        epoch: primary.epoch.clone(),
        operation: step.operation().clone(),
        kind: kind.registration().id.into(),
        schema: kind.registration().schema,
        after: Sequence(after),
        through: Sequence(after + records.len() as u64),
        records,
    };
    let next = checkpoint(primary, step, after * 100 + 100, None);
    let dirty =
        DirtyProjection { kind: REGISTRY.projection("derived.timeline")?, object: primary.custody.object.clone() };
    sources
        .ingest(IngestOperation::new(
            step.clone(),
            expected,
            next.clone(),
            vec![IngestBatch {
                target: bound.clone(),
                facts: Some(IngestFacts::Local { batch }),
                effects: SourceEffects::new(Vec::new(), vec![dirty], Vec::new())?,
            }],
        )?)
        .await?;
    Ok(next)
}

/// Runs content and installation work to quiescence; returns the passes.
async fn settle(db: &Database, blocks: &SqliteBlocks, timelines: &SqliteTimelines) -> Result<u32> {
    let scheduler = SqliteScheduler::new(db.clone(), operation(223), holder(), Duration::from_secs(10))?;
    let mut installs = 0;
    for _ in 0..256 {
        let leases = scheduler.due(now_millis(), 16).await?;
        if leases.is_empty() {
            return Ok(installs);
        }
        for lease in leases {
            let outcome = if lease.task.key.kind == "sync.projection" {
                installs += 1;
                timelines.process(lease.clone()).await?
            } else {
                blocks.process(lease.clone()).await?
            };
            scheduler.finish(lease, outcome).await?;
        }
    }
    Err("timeline work did not settle".into())
}

fn head(db: &Database) -> Result<(u64, Option<String>, Option<u64>, u64)> {
    Ok(db.read_now(|db| {
        Ok(db.query_row("SELECT projection,root,indexed,applied FROM sync_timeline_heads", [], |row| {
            let counter = |bytes: Vec<u8>| u64::from_be_bytes(bytes.try_into().unwrap());
            Ok((
                counter(row.get(0)?),
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?.map(counter),
                counter(row.get(3)?),
            ))
        })?)
    })?)
}

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("timeline-install");
    fastctx_platform::ensure_private_dir(&dir)?;
    let db = open(&dir.join("replica.sqlite3"))?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(95), 1 << 24)?;
    let gate = Arc::new(Gate(AtomicUsize::new(0)));
    let sources = SqliteSources::new(blocks.clone(), Arc::new(Signer), vec![gate as Arc<dyn SourceAuthorizer>])?;
    let timelines = SqliteTimelines::new(blocks.clone());
    let primary = target(&custody(95, "conversation.timeline"), 95);
    let conversation = primary.custody.object.object.clone();
    let generation = HostGeneration::from_bytes([95; 32]);
    let turn = ObjectId::from_bytes([96; 32]);

    // First step: an observed message and the item revision it produces.
    let step = sources.begin_step(primary.clone(), None, generation.clone()).await?;
    let bound = sources.bind_target(step.clone(), primary.clone()).await?;
    let (shown, part, _, descriptor) = display(&sources, &bound, b"\x89PNG first").await?;
    let original = ContentRef::from(block(&sources, &bound, br#"{"uuid":"u-1"}"#).await?);
    let body_root = body(&sources, &bound, original.clone(), shown.clone()).await?;
    let first = identity(&conversation, "u-1")?;
    let item = TimelineItem {
        id: first.id(),
        revision: Revision(1),
        turn: turn.clone(),
        parent: None,
        position: TimelinePosition {
            turn: Sequence(1),
            display_ancestry: vec![Sequence(1)],
            accepted: Sequence(1),
            item: first.id(),
        },
        body: body_root.clone(),
        display: Box::new(descriptor.clone()),
    };
    let facts = vec![observed(first.clone(), 1, original, shown), put(item.clone(), part)];
    let done = commit(&sources, &step, &bound, &primary, None, 0, facts).await?;
    assert_eq!(settle(&db, &blocks, &timelines).await?, 1);
    let (projection, root_one, indexed, applied) = head(&db)?;
    assert_eq!((projection, indexed), (1, Some(1)));
    assert!(applied > 0, "installation caught up with the source commit");
    let root_one = root_one.ok_or("first snapshot has no root")?;
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_timeline_items WHERE active=1")?, 1);
    let installed: TimelineItem = serde_json::from_slice(&db.read_now(|db| {
        Ok(db.query_row("SELECT body FROM sync_timeline_items WHERE active=1", [], |row| row.get::<_, Vec<u8>>(0))?)
    })?)?;
    assert_eq!(installed, item);
    assert_eq!(
        count(&db, &format!("SELECT COUNT(*) FROM sync_asset_manifests WHERE hash='{root_one}' AND complete=1"))?,
        1
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM sync_asset_access WHERE hash='{root_one}' AND object='{}'",
                conversation.as_str()
            )
        )?,
        1
    );
    // The installer's own staging is released once the head holds its pages.
    let staged = "SELECT COUNT(*) FROM sync_asset_pins p JOIN sync_asset_uploads u USING(hub,scope,operation,object) WHERE p.phase!='released' AND u.source IS NULL";
    assert_eq!(count(&db, staged)?, 0);

    // Second step: input facts alone advance the installed cursor, nothing else.
    let second = sources.begin_step(primary.clone(), Some(step.revision()), generation.clone()).await?;
    let bound = sources.bind_target(second.clone(), primary.clone()).await?;
    let (shown_two, part_two, _, descriptor_two) = display(&sources, &bound, b"\x89PNG second").await?;
    let original_two = ContentRef::from(block(&sources, &bound, br#"{"uuid":"u-2"}"#).await?);
    let other = identity(&conversation, "u-2")?;
    let input = observed(other.clone(), 3, original_two.clone(), shown_two.clone());
    let done = commit(&sources, &second, &bound, &primary, Some(done), 2, vec![input]).await?;
    assert_eq!(settle(&db, &blocks, &timelines).await?, 1);
    let (projection, root, _, _) = head(&db)?;
    assert_eq!((projection, root.as_deref()), (1, Some(root_one.as_str())));

    // Third step: the first item is retracted and the second appears.
    // Its body page is realized by the step that publishes the revision; the
    // descriptor and its image were adopted with the observation before.
    let third = sources.begin_step(primary.clone(), Some(second.revision()), generation.clone()).await?;
    let bound = sources.bind_target(third.clone(), primary.clone()).await?;
    let body_two = body(&sources, &bound, original_two, shown_two).await?;
    let item_two = TimelineItem {
        id: other.id(),
        revision: Revision(1),
        turn: turn.clone(),
        parent: None,
        position: TimelinePosition {
            turn: Sequence(1),
            display_ancestry: vec![Sequence(3)],
            accepted: Sequence(3),
            item: other.id(),
        },
        body: body_two,
        display: Box::new(descriptor_two),
    };
    let retract = Fact {
        fact: TimelineFact::ItemRevision {
            change: TimelineChange::Retract { item: first.id(), revision: Revision(2) },
        },
        roots: Vec::new(),
    };
    commit(&sources, &third, &bound, &primary, Some(done), 3, vec![retract, put(item_two.clone(), part_two)]).await?;
    assert_eq!(settle(&db, &blocks, &timelines).await?, 1);
    let (projection, root_two, indexed, _) = head(&db)?;
    let root_two = root_two.ok_or("second snapshot has no root")?;
    assert_eq!((projection, indexed), (2, Some(2)));
    assert_ne!(root_two, root_one);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_timeline_items WHERE active=1")?, 1);
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM sync_timeline_items WHERE active=1 AND item='{}'", other.id().as_str())
        )?,
        1
    );
    // Nothing new: another pass changes nothing.
    assert_eq!(settle(&db, &blocks, &timelines).await?, 0);
    // The superseded root is retained only by the head it no longer is.
    while blocks.drain_steps(64).await? != 0 {}
    while blocks.collect(64).await? != 0 {}
    assert_eq!(count(&db, &format!("SELECT COUNT(*) FROM sync_asset_manifests WHERE hash='{root_one}'"))?, 0);
    assert_eq!(count(&db, &format!("SELECT COUNT(*) FROM sync_asset_manifests WHERE hash='{root_two}'"))?, 1);
    println!(
        "timeline install: item revisions to snapshot and index, input-only passes, retraction, idempotent rerun and head retention passed"
    );
    Ok(())
}
