//! Reversed wake order and delayed observations preserve eventual progress.
use super::*;

pub(super) async fn run(root: &Path) -> Result<()> {
    let path = root.join("dependency-races.sqlite3");
    let db = open(&path, true, 1)?;
    let queue = queue(&db, 20)?;
    queue.dirty(task("late-block", 1, 1)).await?;
    let lease = queue.due(0, 1).await?.pop().unwrap();
    queue.dependency_changed("restored".into()).await?;
    queue
        .finish(lease, Outcome::Blocked { dependency: "restored".into(), reason: "observed before restoration".into() })
        .await?;
    let again = queue.due(1, 1).await?.pop().unwrap();
    queue.finish(again, Outcome::Completed { through: LocalSequence(1) }).await?;

    let mut latest = task("delayed-observation", 2, 1);
    latest.dependency = Some("new-peer".into());
    queue.dirty(latest.clone()).await?;
    let mut old = task("delayed-observation", 1, 1);
    old.dependency = Some("old-peer".into());
    queue.dirty(old).await?;
    assert_eq!(
        db.read_now(|db| Ok(db.query_row(
            "SELECT dependency FROM sync_tasks WHERE task_key='delayed-observation'",
            [],
            |r| r.get::<_, String>(0)
        )?))?,
        "new-peer"
    );
    latest.dependency_revision = LocalSequence(3);
    latest.dependency = None;
    queue.dirty(latest.clone()).await?;
    let mut old = latest.clone();
    old.dependency_revision = LocalSequence(2);
    old.dependency = Some("old-peer".into());
    queue.dirty(old).await?;
    let lease = queue.due(2, 1).await?.pop().unwrap();
    latest.dependency_revision = LocalSequence(4);
    queue.dirty(latest).await?;
    queue
        .finish(lease, Outcome::Blocked { dependency: "stale-worker".into(), reason: "obsolete observation".into() })
        .await?;
    let again = queue.due(3, 1).await?.pop().unwrap();
    assert!(again.task.dependency.is_none());
    queue.finish(again, Outcome::Completed { through: LocalSequence(2) }).await?;

    queue.dirty(task("slow-recheck", 1, 1)).await?;
    let lease = queue.due(4, 1).await?.pop().unwrap();
    queue
        .finish(lease, Outcome::Blocked { dependency: "missed-notification".into(), reason: "no event follows".into() })
        .await?;
    let deadline = db.read_now(|db| {
        Ok(db.query_row("SELECT next_due FROM sync_tasks WHERE task_key='slow-recheck'", [], |r| r.get::<_, i64>(0))?)
    })?;
    assert!(queue.due(deadline - 1, 1).await?.is_empty());
    drop(queue);
    drop(db);
    closed(&path).await?;
    let db = open(&path, true, 1)?;
    let queue = super::queue(&db, 21)?;
    let retry = queue.due(deadline, 1).await?.pop().unwrap();
    assert_eq!(retry.task.key.key, "slow-recheck");
    queue
        .finish(retry, Outcome::Blocked { dependency: "missed-notification".into(), reason: "still absent".into() })
        .await?;
    let later = db.read_now(|db| {
        Ok(db.query_row("SELECT next_due FROM sync_tasks WHERE task_key='slow-recheck'", [], |r| r.get::<_, i64>(0))?)
    })?;
    assert!(later > deadline);
    queue.dependency_changed("missed-notification".into()).await?;
    let final_lease = queue.due(0, 1).await?.pop().unwrap();
    queue.finish(final_lease, Outcome::Completed { through: LocalSequence(1) }).await?;
    Ok(())
}
