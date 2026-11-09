//! Engine-issued steps own their uploads across restarts, supersession and cleanup.
use super::*;
use fastctx_sync::store::{
    access::WorkspaceBinding,
    ingest::SourceTarget,
    source_step::{CancelProgress, SourceStep},
};

pub(super) fn target(owner: &Custody, n: u8) -> SourceTarget {
    let mut custody = owner.clone();
    custody.purpose = HoldingUse::WorkspaceSource {
        workspace: WorkspaceBinding {
            conversation: owner.object.clone(),
            account: AccountId::from_bytes([n; 32]),
            machine: DeviceId::from_bytes([3; 32]),
            epoch: WorkspaceEpoch::from_bytes([n; 32]),
            registration: operation(n),
        },
    };
    SourceTarget {
        source: SourceId::from_bytes([n; 32]),
        custody,
        stream: StreamId::from_bytes([n; 32]),
        epoch: ProducerEpoch::from_bytes([n; 32]),
        producer: Producer::Device { device: DeviceId::from_bytes([3; 32]) },
    }
}
fn steps(db: &Database, state: &str) -> Result<u32> {
    Ok(db.read_now(|db| {
        Ok(db.query_row("SELECT COUNT(*) FROM sync_source_steps WHERE state=?1", [state], |row| row.get(0))?)
    })?)
}
async fn upload(blocks: &SqliteBlocks, step: &SourceStep, target: &SourceTarget, text: &[u8]) -> Result<()> {
    let bound = blocks.bind_target(step.clone(), target.clone()).await?;
    let content = codec::encode(scope(), text.to_vec(), Encoding::Raw)?;
    blocks.put_source(bound, content.representation, content.bytes).await?;
    Ok(())
}

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("source-steps");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(70), 1 << 23)?;
    let primary = target(&custody(70, "conversation.artifact"), 70);
    let generation = HostGeneration::from_bytes([70; 32]);
    assert!(blocks.begin_step(primary.clone(), Some(Revision(5)), generation.clone()).await.is_err());
    let first = blocks.begin_step(primary.clone(), None, generation.clone()).await?;
    assert_eq!(first.revision(), Revision(1));
    upload(&blocks, &first, &primary, b"first attempt").await?;

    // A restart reopens the identical active step and its operation.
    drop(blocks);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let db = open(&path)?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(70), 1 << 23)?;
    let reopened = blocks.begin_step(primary.clone(), None, generation.clone()).await?;
    assert_eq!((reopened.revision(), reopened.operation()), (first.revision(), first.operation()));

    // A destination serves one active step at a time.
    let other = target(&custody(71, "conversation.artifact"), 71);
    let rival = blocks.begin_step(other.clone(), None, generation.clone()).await?;
    let mut contested = other.clone();
    contested.custody = primary.custody.clone();
    contested.source = primary.source.clone();
    assert!(blocks.bind_target(rival.clone(), contested).await.is_err());

    // A different host generation supersedes the active step; its number is spent.
    let second = blocks.begin_step(primary.clone(), None, HostGeneration::from_bytes([73; 32])).await?;
    assert_eq!(second.revision(), Revision(2));
    assert_ne!(second.operation(), first.operation());
    assert!(upload(&blocks, &first, &primary, b"late put on a superseded step").await.is_err());
    assert_eq!(steps(&db, "cancelled")?, 1);
    let mut drained = 0;
    loop {
        match blocks.drain_steps(1).await? {
            0 => break,
            released => drained += released,
        }
    }
    assert_eq!(drained, 1);
    assert!(matches!(blocks.cancel_step(first.clone(), 8).await?, CancelProgress::Done));

    // A committed checkpoint at or past an active step retires it at startup.
    upload(&blocks, &second, &primary, b"second attempt").await?;
    drop(blocks);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let source = primary.source.clone();
    let db = open_with(&path, move |db| {
        db.execute(
            "INSERT INTO sync_sources(hub,scope,source,generation,revision,committed_offset,parser_version,state) VALUES(?1,?2,?3,?4,?5,?6,1,NULL)",
            params![
                scope().hub.to_string(),
                scope().scope.as_str(),
                source.as_str(),
                "g-0000000000000000000000000000000000000000000000000000000000000000",
                2_u64.to_be_bytes().as_slice(),
                0_u64.to_be_bytes().as_slice()
            ],
        )?;
        Ok(())
    })?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(70), 1 << 23)?;
    let mut cursor = None;
    loop {
        cursor = blocks.reconcile_steps(cursor, 1).await?;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(steps(&db, "active")?, 1);
    assert!(matches!(blocks.cancel_step(second, 8).await?, CancelProgress::Draining { released: 1 }));
    assert!(blocks.begin_step(primary.clone(), None, generation.clone()).await.is_err());
    let third = blocks.begin_step(primary, Some(Revision(2)), generation).await?;
    assert_eq!(third.revision(), Revision(3));
    assert!(matches!(blocks.cancel_step(rival, 8).await?, CancelProgress::Done));
    assert!(matches!(blocks.cancel_step(third, 8).await?, CancelProgress::Done));
    assert_eq!(blocks.drain_steps(8).await?, 0);
    println!("source steps: reopen, restart, exclusive destinations, supersession, reconciliation and drain passed");
    Ok(())
}
