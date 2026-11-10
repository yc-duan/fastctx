//! Readers race real installations; undisclosed holds end before Pending or errors return.
use super::*;
use fastctx_sync::store::{
    Error,
    roots::{AcquireRoot, AcquireRootError, RootStore, RootUse},
};
use std::collections::BTreeMap;
use tokio::sync::Barrier;

struct PostReadPolicy {
    base: Arc<super::super::Policy>,
    armed: AtomicBool,
    reads: AtomicUsize,
}
impl AccessPolicy for PostReadPolicy {
    fn can_read(&self, context: &AccessContext, kind: RegisteredKind) -> Decision {
        self.base.can_read(context, kind)
    }
    fn can_read_object(&self, context: &AccessContext, kind: RegisteredKind, object: &ObjectKey) -> Decision {
        if self.armed.load(Ordering::Acquire) && self.reads.fetch_add(1, Ordering::AcqRel) != 0 {
            Decision::Denied { revision: Revision(1), reason: "reader lost access before returning its page".into() }
        } else {
            self.base.can_read_object(context, kind, object)
        }
    }
    fn can_write(&self, context: &AccessContext, kind: RegisteredKind) -> Decision {
        self.base.can_write(context, kind)
    }
    fn can_keep(
        &self,
        context: &AccessContext,
        kind: RegisteredKind,
        object: &ObjectKey,
        usage: &HoldingUse,
    ) -> Decision {
        self.base.can_keep(context, kind, object, usage)
    }
    fn can_execute(&self, context: &AccessContext, device: &DeviceId) -> Decision {
        self.base.can_execute(context, device)
    }
}

struct Committed {
    projection: Revision,
    local: LocalSequence,
    applied: LocalSequence,
    through: StreamCursor,
    item_revision: Revision,
}
fn remember(db: &Database, revision: Revision) -> Result<(Digest, Committed)> {
    let (projection, root, local, applied, through) = db.read_now(|db| {
        Ok(db.query_row("SELECT projection,root,local,applied,through_cursor FROM sync_timeline_heads", [], |row| {
            let counter = |bytes: Vec<u8>| u64::from_be_bytes(bytes.try_into().unwrap());
            Ok((
                counter(row.get(0)?),
                row.get::<_, String>(1)?,
                counter(row.get(2)?),
                counter(row.get(3)?),
                row.get::<_, Vec<u8>>(4)?,
            ))
        })?)
    })?;
    Ok((
        Digest::try_from(root)?,
        Committed {
            projection: Revision(projection),
            local: LocalSequence(local),
            applied: LocalSequence(applied),
            through: serde_json::from_slice(&through)?,
            item_revision: revision,
        },
    ))
}
fn request(primary: &SourceTarget, fence: Option<TimelineFence>) -> TimelineRead {
    TimelineRead {
        conversation: primary.custody.object.clone(),
        fence,
        anchor: TimelineAnchor::Latest,
        max_items: 100,
        max_bytes: 262144,
    }
}
async fn page(
    timelines: &SqliteTimelines,
    primary: &SourceTarget,
) -> Result<Box<fastctx_sync::timeline::TimelinePage>> {
    match timelines.timeline(primary.custody.context.clone(), request(primary, None)).await? {
        TimelineResult::Page { page } => Ok(page),
        other => Err(format!("complete timeline did not return a page: {other:?}").into()),
    }
}

pub(super) async fn run(root: &Path) -> Result<()> {
    let directory = root.join("timeline-fresh");
    fastctx_platform::ensure_private_dir(&directory)?;
    let path = directory.join("replica.sqlite3");
    let db = open(&path)?;
    let access =
        Arc::new(PostReadPolicy { base: policy(false), armed: AtomicBool::new(false), reads: AtomicUsize::new(0) });
    let blocks =
        SqliteBlocks::new(db.clone(), directory.join("files"), holder(), access.clone(), operation(235), 1 << 24)?;
    let sources = SqliteSources::new(
        blocks.clone(),
        Arc::new(Signer),
        vec![Arc::new(Gate(AtomicUsize::new(0)))],
        IngestBudget::for_machine(None)?,
    )?;
    let timelines = SqliteTimelines::new(blocks.clone());
    let primary = target(&custody(235, "conversation.timeline"), 235);
    let generation = HostGeneration::from_bytes([235; 32]);
    let mut step = sources.begin_step(primary.clone(), None, generation.clone()).await?;
    let bound = sources.bind_target(step.clone(), primary.clone()).await?;
    let (shown, part, _, descriptor) = display(&sources, &bound, b"fresh snapshot image").await?;
    let original = ContentRef::from(block(&sources, &bound, b"fresh source evidence").await?);
    let item_id = identity(&primary.custody.object.object, "fresh-race")?.id();
    let mut item = TimelineItem {
        id: item_id.clone(),
        revision: Revision(1),
        turn: None,
        parent: None,
        position: TimelinePosition {
            turn: Sequence(0),
            display_ancestry: vec![Sequence(1)],
            accepted: Sequence(1),
            item: item_id,
        },
        body: body(&sources, &bound, original, shown).await?,
        display: Box::new(descriptor),
    };
    let mut checkpoint =
        commit(&sources, &step, &bound, &primary, None, 0, vec![put(item.clone(), part.clone())]).await?;
    settle(&db, &blocks, &timelines).await?;
    let mut committed = BTreeMap::from([remember(&db, item.revision)?]);
    let first = page(&timelines, &primary).await?;
    timelines.release(primary.custody.context.clone(), first.fence.clone()).await?;

    step = sources.begin_step(primary.clone(), Some(step.revision()), generation.clone()).await?;
    let bound = sources.bind_target(step.clone(), primary.clone()).await?;
    item.revision = Revision(2);
    checkpoint =
        commit(&sources, &step, &bound, &primary, Some(checkpoint), 1, vec![put(item.clone(), part.clone())]).await?;
    settle(&db, &blocks, &timelines).await?;
    committed.extend([remember(&db, item.revision)?]);
    let acquisition = |fence: &TimelineFence| AcquireRoot {
        lease: RootLeaseId::from_bytes([237; 32]),
        custody: primary.custody.clone(),
        root: ContentRef::Manifest { root: fence.root.clone() },
        usage: RootUse::Timeline { projection: fence.projection },
        bases: Vec::new(),
        expires_at_millis: Some(now_millis() + 60000),
    };
    assert!(
        matches!(
            blocks.acquire_root(operation(237), acquisition(&first.fence)).await,
            Err(AcquireRootError::TimelineUnbound)
        ),
        "a superseded unheld root has a typed acquisition result"
    );
    assert!(
        matches!(Error::from(AcquireRootError::TimelineUnbound), Error::Invalid(_)),
        "ordinary acquisition callers retain their invalid-request mapping"
    );
    let current = page(&timelines, &primary).await?;
    let mut invalid = acquisition(&current.fence);
    invalid.usage = RootUse::Timeline { projection: Revision(999) };
    assert!(matches!(
        blocks.acquire_root(operation(237), invalid).await,
        Err(AcquireRootError::Store(Error::Invalid(_)))
    ));
    let mut invalid = acquisition(&current.fence);
    invalid.expires_at_millis = Some(0);
    assert!(matches!(
        blocks.acquire_root(operation(237), invalid).await,
        Err(AcquireRootError::Store(Error::Invalid(_)))
    ));
    let mut invalid = acquisition(&current.fence);
    invalid.root = item.body.clone();
    assert!(matches!(
        blocks.acquire_root(operation(237), invalid).await,
        Err(AcquireRootError::Store(Error::Invalid(_)))
    ));
    timelines.release(primary.custody.context.clone(), current.fence).await?;

    let finished = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(5));
    let mut readers = Vec::new();
    for _ in 0..4 {
        let (read, context, primary, done, start) =
            (timelines.clone(), primary.custody.context.clone(), primary.clone(), finished.clone(), start.clone());
        readers.push(tokio::spawn(async move {
            start.wait().await;
            let mut pages = Vec::new();
            let mut busy = 0;
            for _ in 0..512 {
                match read.timeline(context.clone(), request(&primary, None)).await {
                    Ok(TimelineResult::Page { page }) => {
                        read.release(context.clone(), page.fence.clone()).await?;
                        pages.push(page);
                    }
                    Err(Error::Busy) => busy += 1,
                    Err(error) => return Err(error),
                    Ok(other) => return Err(Error::Corrupt(format!("complete snapshot returned {other:?}"))),
                }
                if done.load(Ordering::Acquire) && pages.len() >= 64 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            Ok::<_, Error>((pages, busy))
        }));
    }
    start.wait().await;
    let installation = async {
        for after in 2..14 {
            step = sources.begin_step(primary.clone(), Some(step.revision()), generation.clone()).await?;
            let bound = sources.bind_target(step.clone(), primary.clone()).await?;
            item.revision = Revision(after + 1);
            checkpoint = commit(
                &sources,
                &step,
                &bound,
                &primary,
                Some(checkpoint.clone()),
                after,
                vec![put(item.clone(), part.clone())],
            )
            .await?;
            settle(&db, &blocks, &timelines).await?;
            committed.extend([remember(&db, item.revision)?]);
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    finished.store(true, Ordering::Release);
    let (mut reads, mut busy) = (0, 0);
    for reader in readers {
        let (pages, retries) = reader.await??;
        assert!(pages.len() >= 64);
        reads += pages.len();
        busy += retries;
        for page in pages {
            let head = committed.get(&page.fence.root.0.digest).ok_or("fence root was never committed")?;
            assert_eq!(
                (page.fence.projection, page.fence.local, page.fence.applied),
                (head.projection, head.local, head.applied)
            );
            assert_eq!(page.fence.through.cursor(), &head.through);
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].revision, head.item_revision);
        }
    }
    installation?;
    access.armed.store(true, Ordering::Release);
    let denied = timelines.timeline(primary.custody.context.clone(), request(&primary, None)).await;
    access.armed.store(false, Ordering::Release);
    assert!(matches!(denied, Err(Error::Denied(_))));
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM sync_root_leases WHERE released=0 AND supply_id IS NULL")?,
        0,
        "a failed return-time authorization must not retain its undisclosed hold"
    );
    let from = page(&timelines, &primary).await?.fence;
    access.reads.store(0, Ordering::Release);
    access.armed.store(true, Ordering::Release);
    let denied = timelines
        .changes(
            primary.custody.context.clone(),
            fastctx_sync::timeline::TimelineChangesRead {
                from: from.clone(),
                to: None,
                after: None,
                max_items: 100,
                max_bytes: 262144,
            },
        )
        .await;
    access.armed.store(false, Ordering::Release);
    assert!(matches!(denied, Err(Error::Denied(_))));
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM sync_root_leases WHERE released=0 AND supply_id IS NULL")?,
        1,
        "a failed diff authorization must release only its undisclosed target hold"
    );
    timelines.release(primary.custody.context.clone(), from).await?;
    let held = page(&timelines, &primary).await?.fence;
    let hash = held.root.0.digest.as_str().to_owned();
    drop((sources, timelines, blocks, db));
    super::super::super::scheduler::closed(&path).await?;
    let db = open_with(&path, |db| {
        db.execute("UPDATE sync_timeline_heads SET indexed=NULL", [])?;
        let removed = db.execute("DELETE FROM sync_asset_manifests WHERE hash IN (SELECT child FROM sync_asset_edges WHERE parent=?1 AND child_kind='manifest')", [&hash])?;
        assert!(removed > 0, "the pending fixture must really lose immutable pages");
        Ok(())
    })?;
    let blocks =
        SqliteBlocks::new(db.clone(), directory.join("files"), holder(), policy(false), operation(235), 1 << 24)?;
    let timelines = SqliteTimelines::new(blocks);
    let active = "SELECT COUNT(*) FROM sync_root_leases WHERE released=0 AND supply_id IS NULL";
    let total = count(&db, "SELECT COUNT(*) FROM sync_root_leases")?;
    assert_eq!(count(&db, active)?, 1);
    assert!(matches!(
        timelines.timeline(primary.custody.context.clone(), request(&primary, Some(held.clone()))).await?,
        TimelineResult::Pending { .. }
    ));
    assert_eq!(count(&db, active)?, 1, "a supplied fence remains the caller's hold");
    for _ in 0..3 {
        assert!(matches!(
            timelines.timeline(primary.custody.context.clone(), request(&primary, None)).await?,
            TimelineResult::Pending { .. }
        ));
        assert_eq!(count(&db, active)?, 1, "Pending must immediately release its undisclosed fresh hold");
    }
    assert!(count(&db, "SELECT COUNT(*) FROM sync_root_leases")? >= total + 3);
    let mut invalid = request(&primary, None);
    invalid.max_items = 0;
    assert!(matches!(timelines.timeline(primary.custody.context.clone(), invalid).await, Err(Error::Invalid(_))));
    assert_eq!(count(&db, active)?, 1, "post-acquisition errors also release their fresh hold");
    timelines.release(primary.custody.context.clone(), held).await?;
    assert_eq!(count(&db, active)?, 0);
    println!(
        "timeline fresh lifecycle: 14 committed heads, {reads} concurrent Latest pages, {busy} bounded Busy retries, zero Invalid; typed stale binding, invalid inputs, Pending and failed-read hold cleanup passed"
    );
    Ok(())
}
