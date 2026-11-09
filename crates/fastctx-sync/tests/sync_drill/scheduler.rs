use fastctx_storage::{Database, rusqlite::params};
use fastctx_sync::{
    cursor::{Holder, HoldingPeriod, LocalSequence, Revision},
    identity::{DeviceId, HubId, OperationId, ScopeId, ScopeKey},
    scheduler::{Outcome, Priority, Scheduler, SqliteScheduler, Task, TaskKey},
    store::{boundary, schema},
};
use std::{path::Path, time::Duration};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn scope() -> ScopeKey {
    ScopeKey { hub: HubId::from_bytes([1; 8]), scope: ScopeId::from_bytes([2; 32]) }
}
fn holder() -> Holder {
    Holder::Device { device: DeviceId::from_bytes([3; 32]) }
}
fn task(key: &str, wanted: u64, publication: u64) -> Task {
    Task {
        key: TaskKey { kind: "sync.manifest".into(), scope: scope(), key: key.into() },
        wanted: LocalSequence(wanted),
        completed: LocalSequence(0),
        priority: Priority::Background,
        next_due_millis: 0,
        dependency: None,
        holding: Some(HoldingPeriod {
            holder: holder(),
            scope: scope(),
            acl_revision: Revision(1),
            publication_epoch: Revision(publication),
        }),
    }
}
fn open(path: &Path, enabled: bool, publication: u64) -> fastctx_storage::Result<Database> {
    Database::open_guarded(path, "", boundary::guard, move |db| {
        schema::initialize(db)?;
        db.execute("INSERT INTO sync_scopes(hub,scope,policy,acl_revision,enabled,publication_epoch) VALUES(?1,?2,X'7b7d',?3,?4,?5) ON CONFLICT(hub,scope) DO UPDATE SET enabled=excluded.enabled,publication_epoch=excluded.publication_epoch",params![scope().hub.to_string(),scope().scope.as_str(),1u64.to_be_bytes().as_slice(),enabled,publication.to_be_bytes().as_slice()])?;
        Ok(())
    })
}
fn queue(db: &Database, worker: u8) -> Result<SqliteScheduler> {
    Ok(SqliteScheduler::new(db.clone(), OperationId::from_bytes([worker; 32]), holder(), Duration::from_millis(50))?)
}
pub(super) async fn closed(path: &Path) -> Result<()> {
    let keeper = path.with_file_name(format!("{}-keeper", path.file_name().unwrap().to_string_lossy()));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if fastctx_platform::lock::FileLock::try_acquire(&keeper)?.is_some() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("database writer did not close".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
fn completed(db: &Database, key: &str) -> u64 {
    db.read_now(|db| {
        let bytes: Vec<u8> =
            db.query_row("SELECT completed FROM sync_tasks WHERE task_key=?1", [key], |row| row.get(0))?;
        Ok(u64::from_be_bytes(bytes.try_into().unwrap()))
    })
    .unwrap()
}
pub async fn run(root: &Path) -> Result<()> {
    let path = root.join("scheduler.sqlite3");
    let db = open(&path, true, 1)?;
    let a = queue(&db, 10)?;
    let b = queue(&db, 11)?;
    a.dirty(task("coalesced", 1, 1)).await?;
    let first = a.due(0, 8).await?.pop().unwrap();
    a.dirty(task("coalesced", 2, 1)).await?;
    let mut forged = first.clone();
    forged.task.wanted = LocalSequence(2);
    assert!(a.finish(forged, Outcome::Completed { through: LocalSequence(2) }).await.is_err());
    assert_eq!(completed(&db, "coalesced"), 0);
    assert!(b.due(1, 8).await?.is_empty());
    a.finish(first, Outcome::Completed { through: LocalSequence(1) }).await?;
    let second = b.due(2, 8).await?.pop().unwrap();
    assert_eq!(second.task.wanted, LocalSequence(2));
    b.finish(second, Outcome::Completed { through: LocalSequence(2) }).await?;
    assert_eq!(completed(&db, "coalesced"), 2);
    assert!(a.due(100, 8).await?.is_empty());

    a.dirty(task("expired", 1, 1)).await?;
    let stale = a.due(100, 8).await?.pop().unwrap();
    let replacement = b.due(151, 8).await?.pop().unwrap();
    assert_ne!(stale.generation, replacement.generation);
    a.finish(stale, Outcome::Completed { through: LocalSequence(1) }).await?;
    assert_eq!(completed(&db, "expired"), 0);
    b.finish(replacement, Outcome::Blocked { dependency: "peer-online".into(), reason: "disconnected".into() }).await?;
    assert!(a.due(1000, 8).await?.is_empty());
    a.dependency_changed("peer-online".into()).await?;
    let retried = a.due(1001, 8).await?.pop().unwrap();
    a.dirty(task("expired", 2, 1)).await?;
    a.finish(retried, Outcome::Failed { reason: "old input rejected".into() }).await?;
    let latest = a.due(1002, 8).await?.pop().unwrap();
    assert_eq!(latest.task.wanted, LocalSequence(2));
    a.finish(latest, Outcome::Completed { through: LocalSequence(2) }).await?;

    let mut background = task("priority", 1, 1);
    background.next_due_millis = 3000;
    a.dirty(background.clone()).await?;
    background.priority = Priority::Control;
    background.next_due_millis = 2000;
    a.dirty(background).await?;
    assert!(a.due(1999, 8).await?.is_empty());
    let prioritized = a.due(2000, 1).await?.pop().unwrap();
    assert!(matches!(prioritized.task.priority, Priority::Control));
    a.finish(prioritized, Outcome::Completed { through: LocalSequence(1) }).await?;

    let mut unknown = task("unregistered", 1, 1);
    unknown.key.kind = "random.channel".into();
    assert!(a.dirty(unknown).await.is_err());
    assert!(
        db.write(128, |db| {
            db.execute("DELETE FROM sync_tasks", [])?;
            Ok(())
        })
        .await
        .is_err()
    );
    assert!(
        db.write(128, |db| {
            let _ = db.execute_batch("CREATE TABLE hidden_asset(body BLOB)");
            Ok(())
        })
        .await
        .is_err()
    );
    assert_eq!(completed(&db, "coalesced"), 2);
    db.read_now(|db| {
        for query in [
            "EXPLAIN QUERY PLAN SELECT task_key FROM sync_tasks WHERE state='ready' AND priority=1 AND next_due<=2000 ORDER BY next_due,hub,scope,kind,task_key LIMIT 8",
            "EXPLAIN QUERY PLAN SELECT task_key FROM sync_tasks WHERE dependency='peer-online' AND state='blocked' ORDER BY next_due LIMIT 8",
        ] {
            let mut statement=db.prepare(query)?;
            let plan=statement.query_map([],|row|row.get::<_,String>(3))?.collect::<std::result::Result<Vec<_>,_>>()?.join("; ");
            assert!(plan.contains("INDEX sync_tasks_"),"{plan}");
            assert!(!plan.contains("TEMP B-TREE"),"{plan}");
        }
        Ok(())
    })?;

    a.dirty(task("restart", 1, 1)).await?;
    let lost = a.due(3000, 8).await?.pop().unwrap();
    drop(a);
    drop(b);
    drop(db);
    closed(&path).await?;
    let db = open(&path, true, 1)?;
    let reopened = queue(&db, 12)?;
    let resumed = reopened.due(3051, 8).await?.pop().unwrap();
    assert_ne!(resumed.generation, lost.generation);
    reopened.finish(lost.clone(), Outcome::Completed { through: LocalSequence(1) }).await?;
    assert_eq!(completed(&db, "restart"), 0);
    drop(reopened);
    drop(db);
    closed(&path).await?;
    let db = open(&path, false, 2)?;
    let revoked = queue(&db, 13)?;
    revoked.finish(resumed, Outcome::Completed { through: LocalSequence(1) }).await?;
    assert_eq!(completed(&db, "restart"), 0);
    assert!(revoked.dirty(task("restart", 2, 1)).await.is_err());
    drop(revoked);
    drop(db);
    closed(&path).await?;
    let db = open(&path, true, 3)?;
    let regranted = queue(&db, 14)?;
    regranted.dirty(task("restart", 1, 3)).await?;
    regranted.finish(lost, Outcome::Completed { through: LocalSequence(1) }).await?;
    let new = regranted.due(4000, 8).await?.pop().unwrap();
    assert_eq!(new.task.holding.as_ref().unwrap().publication_epoch, Revision(3));
    regranted.finish(new, Outcome::Completed { through: LocalSequence(1) }).await?;
    assert_eq!(completed(&db, "restart"), 1);

    for n in 0..300 {
        let mut task = task(&format!("wait-{n}"), 1, 3);
        task.dependency = Some("one-change".into());
        regranted.dirty(task).await?;
    }
    regranted.dependency_changed("one-change".into()).await?;
    let count = db.read_now(|db| {
        Ok(db.query_row("SELECT count(*) FROM sync_tasks WHERE state='ready'", [], |row| row.get::<_, u32>(0))?)
    })?;
    assert_eq!(count, 300);
    let competitor = queue(&db, 15)?;
    let claimant = regranted.clone();
    let other = tokio::spawn(async move { competitor.due(5000, 128).await });
    let ours = claimant.due(5000, 128).await?;
    let theirs = other.await.unwrap()?;
    let identities: std::collections::BTreeSet<_> =
        ours.iter().chain(&theirs).map(|lease| lease.task.key.key.as_str()).collect();
    assert_eq!(ours.len() + theirs.len(), 256);
    assert_eq!(identities.len(), 256);
    for n in 0..16 {
        let mut control = task(&format!("control-{n}"), 1, 3);
        control.priority = Priority::Control;
        regranted.dirty(control).await?;
    }
    let mut background_claimed = false;
    for _ in 0..8 {
        let lease = regranted.due(6000, 1).await?.pop().unwrap();
        background_claimed |= matches!(lease.task.priority, Priority::Background);
    }
    assert!(background_claimed, "continuous control work starved background progress");
    assert_eq!(
        db.read_now(|db| {
            let value: Vec<u8> =
                db.query_row("SELECT sequence FROM sync_local_head WHERE singleton=1", [], |row| row.get(0))?;
            Ok(value)
        })?,
        0u64.to_be_bytes()
    );
    println!(
        "scheduler lifecycle: coalescing, leases, restart, holding fences, bounded wake and architecture refusals passed"
    );
    Ok(())
}
