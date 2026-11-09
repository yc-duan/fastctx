//! Original proofs, receiving cursors and content resume across independent roots.
use super::*;
use fastctx_sync::{
    catalog::{
        Authority, CatalogChange, ChangesPage, FeedEntry, HeadEvidence, Lifecycle, ObjectHead, RecordRef, SourceBatch,
        SourceRecord,
    },
    cursor::{Completeness, ScopeCursor, Sequence, StreamCursor, VersionRef},
    manifest::{ContentBlock, ContentRef},
    proof::{ConfirmedEpoch, EpochDescriptor, HubSigningIdentity, SignedSourceBatch},
    store::{
        Error,
        replica::{ReplicaContext, ReplicaSink, SqliteReplica, StreamInstalled},
        timelines::SqliteTimelines,
    },
    timeline::{
        Direction, ProvenRecord, StreamPage, TimelineAnchor, TimelineChange, TimelineItem, TimelinePosition,
        TimelineRead, TimelineReader, TimelineResult,
    },
};
use fastctx_types::sync::{
    conversation_timeline::{Evidence, TimelineEvent, TimelineFact},
    feed,
    progress::StreamProgress,
    transfer::{KindVersion, ViewDefinition},
};
use std::collections::BTreeMap;
#[path = "replica/crash.rs"]
mod crash;
#[path = "replica/fixture.rs"]
mod fixture;
pub(crate) use crash::child;
use fixture::{Fixture, Verifier};

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("replica-install");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(92), 1 << 24)?;
    let f = Fixture::new()?;
    let receiver = SqliteReplica::new(blocks.clone(), Arc::new(Verifier), f.hub.clone());
    receiver.begin(f.context.clone(), f.begin.clone(), f.view.clone()).await?;
    let mut wrong = f.changes.clone();
    wrong.after.through = Sequence(1);
    assert!(receiver.apply_changes(f.context.clone(), wrong).await.is_err());
    let mut wrong = f.changes.clone();
    if let FeedEntry::Changed { prefix_digest, .. } = &mut wrong.entries[0] {
        *prefix_digest = Digest::of(b"bad prefix");
    }
    assert!(receiver.apply_changes(f.context.clone(), wrong).await.is_err());
    assert_eq!(receiver.cursor(f.context.clone()).await?, Some(f.begin.clone()));
    let applied = receiver.apply_changes(f.context.clone(), f.changes.clone()).await?;
    let replay = receiver.apply_changes(f.context.clone(), f.changes.clone()).await?;
    assert_eq!(replay.local, applied.local);
    assert!(matches!(
        receiver.apply_stream(f.context.clone(), f.stream.clone()).await?,
        StreamInstalled::AwaitingEpoch { .. }
    ));
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_stream_records")?, 0);
    // Closing and reopening does not require a rescan or a producer checkpoint.
    drop((receiver, blocks, db));
    super::super::scheduler::closed(&path).await?;
    let db = open(&path)?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(92), 1 << 24)?;
    let receiver = SqliteReplica::new(blocks.clone(), Arc::new(Verifier), f.hub.clone());
    receiver.install_epoch(f.context.clone(), f.epoch.clone()).await?;
    let mut forged = f.stream.clone();
    forged.proofs[0].proof.signature.0[0] ^= 1;
    assert!(receiver.apply_stream(f.context.clone(), forged).await.is_err());
    let mut gap = f.stream.clone();
    gap.records.remove(0);
    assert!(receiver.apply_stream(f.context.clone(), gap).await.is_err());
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_stream_records")?, 0);
    let result =
        receiver.resume_stream(f.context.clone(), f.stream.fence.cursor().stream.clone(), f.epoch.proof.id()).await?;
    assert!(matches!(result, Some(StreamInstalled::Committed { progress: StreamProgress::Confirmed { .. }, .. })));
    let again = receiver.apply_stream(f.context.clone(), f.stream.clone()).await?;
    assert!(matches!(again, StreamInstalled::Committed { progress: StreamProgress::Confirmed { .. }, .. }));
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_sources")?, 0);
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_source_steps")?, 0);
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_stream_records")?, 3);
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_stream_heads WHERE origin='replicated'")?, 1);
    let timelines = SqliteTimelines::new(blocks.clone());
    let reader = AccessContext { principal: Principal::LocalService, scope: scope(), observed_acl: Revision(1) };
    let request = TimelineRead {
        conversation: f.object.clone(),
        fence: None,
        anchor: TimelineAnchor::Latest,
        max_items: 100,
        max_bytes: 262144,
    };
    settle(&db, &blocks, &timelines).await?;
    assert!(matches!(timelines.timeline(reader.clone(), request.clone()).await?, TimelineResult::Pending { .. }));
    fetch(&receiver, &f).await?;
    settle(&db, &blocks, &timelines).await?;
    let TimelineResult::Page { page } = timelines.timeline(reader.clone(), request).await? else {
        return Err("replica timeline did not become readable".into());
    };
    assert_eq!(page.items, f.items);
    assert!(matches!(page.fence.through, StreamProgress::Confirmed { .. }));
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_asset_pins WHERE phase!='released'")?, 0);
    let TimelineResult::Page { page: latest } = timelines
        .timeline(
            reader.clone(),
            TimelineRead {
                conversation: f.object.clone(),
                fence: Some(page.fence.clone()),
                anchor: TimelineAnchor::Latest,
                max_items: 1,
                max_bytes: 262144,
            },
        )
        .await?
    else {
        return Err("latest keyset page is missing".into());
    };
    assert_eq!(latest.items, vec![f.items[1].clone()]);
    let TimelineResult::Page { page: older } = timelines
        .timeline(
            reader.clone(),
            TimelineRead {
                conversation: f.object.clone(),
                fence: Some(page.fence.clone()),
                anchor: TimelineAnchor::Before(latest.items[0].position.clone()),
                max_items: 1,
                max_bytes: 262144,
            },
        )
        .await?
    else {
        return Err("older keyset page is missing".into());
    };
    assert_eq!(older.items, vec![f.items[0].clone()]);
    timelines.release(reader, page.fence).await?;
    crash::run(root).await?;
    println!(
        "replica installation: feed refusal/replay, late epoch, signature/gap refusal, durable source evidence, missing content and confirmed timeline passed"
    );
    Ok(())
}
async fn fetch(receiver: &SqliteReplica, f: &Fixture) -> Result<()> {
    let mut after = None;
    for _ in 0..128 {
        let missing = receiver
            .missing_content(f.context.clone(), f.object.clone(), "conversation.timeline".into(), after, 2)
            .await?;
        if !missing.pending {
            return Ok(());
        }
        for request in missing.requests {
            match request.root().clone() {
                ContentRef::Single { block, .. } => {
                    let encoded = codec::encode(scope(), f.blocks[&block.digest].clone(), Encoding::Raw)?;
                    receiver
                        .receive_block(request.clone(), encoded.representation.clone(), encoded.bytes.clone())
                        .await?;
                    receiver.receive_block(request, encoded.representation, encoded.bytes).await?;
                }
                ContentRef::Manifest { root } => {
                    receiver.receive_manifest(request, f.pages[&root.0.digest].clone()).await?
                }
            }
        }
        after = missing.next;
    }
    Err("replica closure did not finish its bounded traversal".into())
}
async fn settle(db: &Database, blocks: &SqliteBlocks, timelines: &SqliteTimelines) -> Result<()> {
    let scheduler = SqliteScheduler::new(db.clone(), operation(222), holder(), Duration::from_secs(10))?;
    for _ in 0..256 {
        let tasks = scheduler.due(now_millis(), 16).await?;
        if tasks.is_empty() {
            return Ok(());
        }
        for task in tasks {
            let outcome = match task.task.key.kind.as_str() {
                "sync.transfer" => fastctx_sync::scheduler::Outcome::Completed { through: task.task.wanted },
                "sync.projection" => fastctx_sync::scheduler::TaskHandler::handle(timelines, task.clone()).await?,
                _ => blocks.process(task.clone()).await?,
            };
            scheduler.finish(task, outcome).await?;
        }
    }
    Err("replica installation tasks did not settle".into())
}
