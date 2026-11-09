//! Scope advances resume bounded maintenance without reviving old-period work.
use super::*;

use fastctx_sync::{
    manifest::ContentRef,
    scheduler::{Lease, Outcome},
    store::{
        Error,
        roots::{AcquireRoot, RootAcquired, RootStore, RootUse},
        scopes::{ScopeState, SqliteScopes},
    },
};
use std::sync::atomic::AtomicU64;

struct Gate(AtomicU64);
impl Gate {
    fn decision(&self) -> Decision {
        Decision::Allowed { revision: Revision(self.0.load(Ordering::Acquire)) }
    }
}
impl AccessPolicy for Gate {
    fn can_read(&self, _: &AccessContext, _: RegisteredKind) -> Decision {
        self.decision()
    }
    fn can_read_object(&self, _: &AccessContext, _: RegisteredKind, _: &ObjectKey) -> Decision {
        self.decision()
    }
    fn can_write(&self, _: &AccessContext, _: RegisteredKind) -> Decision {
        self.decision()
    }
    fn can_keep(&self, _: &AccessContext, _: RegisteredKind, object: &ObjectKey, _: &HoldingUse) -> Decision {
        let revision = Revision(self.0.load(Ordering::Acquire));
        if revision.0 > 1 && object.object == ObjectId::from_bytes([101; 32]) {
            Decision::Denied { revision, reason: "this object's custody ended".into() }
        } else {
            Decision::Allowed { revision }
        }
    }
    fn can_execute(&self, _: &AccessContext, _: &DeviceId) -> Decision {
        self.decision()
    }
}
fn state(acl: u64, epoch: u64) -> ScopeState {
    ScopeState { acl_revision: Revision(acl), publication_epoch: Revision(epoch), enabled: true }
}
fn request(owner: Custody, root: ContentRef, id: u8) -> AcquireRoot {
    AcquireRoot {
        lease: RootLeaseId::from_bytes([id; 32]),
        custody: owner,
        root,
        usage: RootUse::Runtime { kind: "runtime.transport".into(), record_key: format!("use-{id}") },
        bases: vec![],
        expires_at_millis: None,
    }
}
fn store(db: &Database, dir: &Path, gate: Arc<Gate>) -> Result<SqliteBlocks> {
    Ok(SqliteBlocks::new(db.clone(), dir.join("files"), holder(), gate, operation(244), 8 * 1024 * 1024)?)
}
fn scopes(blocks: &SqliteBlocks) -> SqliteScopes {
    SqliteScopes::new(blocks.clone(), scope().hub, DeviceId::from_bytes([3; 32]))
}
fn moved(db: &Database, holding: &HoldingPeriod) -> Result<i64> {
    let bytes = serde_json::to_vec(holding)?;
    Ok(db.read_now(|db| {
        Ok(db.query_row(
            "SELECT COUNT(*) FROM sync_root_leases WHERE released=0 AND supply_id IS NULL AND holding=?1",
            [&bytes],
            |row| row.get(0),
        )?)
    })?)
}
async fn process(blocks: &SqliteBlocks, lease: Lease) -> Result<Outcome> {
    if SqliteBlocks::takes_reconciliation(&lease.task.key) {
        Ok(blocks.reconcile_scope(lease).await?)
    } else {
        Ok(blocks.process(lease).await?)
    }
}
async fn drain(db: &Database, blocks: &SqliteBlocks) -> Result<()> {
    let queue = SqliteScheduler::new(db.clone(), operation(245), holder(), Duration::from_secs(10))?;
    for _ in 0..512 {
        let due = queue.due(now_millis() + 100_000, 8).await?;
        if due.is_empty() {
            return Ok(());
        }
        for lease in due {
            let outcome = process(blocks, lease.clone()).await?;
            queue.finish(lease, outcome).await?;
        }
    }
    Err("holding maintenance did not finish its bounded work".into())
}
pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("holding-transitions");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let gate = Arc::new(Gate(AtomicU64::new(1)));
    let blocks = store(&db, &dir, gate.clone())?;
    let owner = custody(100, "conversation.artifact");
    let encoded = codec::encode(scope(), b"state that survives a directory change".to_vec(), Encoding::Raw)?;
    let body = blocks.put(owner.clone(), operation(100), encoded.representation.clone(), encoded.bytes.clone()).await?;
    let root = ContentRef::from(body.content.clone());
    for id in 1..=70 {
        assert!(matches!(
            blocks.acquire_root(operation(100), request(owner.clone(), root.clone(), id)).await?,
            RootAcquired::Held { .. }
        ));
    }
    let refused = custody(101, "conversation.artifact");
    blocks.put(refused.clone(), operation(101), encoded.representation.clone(), encoded.bytes.clone()).await?;
    assert!(matches!(
        blocks.acquire_root(operation(101), request(refused.clone(), root.clone(), 200)).await?,
        RootAcquired::Held { .. }
    ));
    drain(&db, &blocks).await?;

    // Leave a real, unfinished supply frontier under the original period.
    blocks.put(owner.clone(), operation(110), encoded.representation.clone(), encoded.bytes.clone()).await?;
    let page_json =
        serde_json::to_string(&ManifestPage::Blocks { offset: ByteCount(0), blocks: vec![body.content; 160] })?;
    let page = ManifestReply {
        root: ManifestRoot(BlobId {
            domain: scope(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(page_json.as_bytes()),
        }),
        page_json,
    };
    blocks.put_manifest(owner.clone(), operation(110), page.clone()).await?;
    assert!(matches!(
        blocks
            .acquire_root(operation(110), request(owner.clone(), ContentRef::Manifest { root: page.root.clone() }, 201))
            .await?,
        RootAcquired::Pending { .. }
    ));

    let scope_store = scopes(&blocks);
    gate.0.store(2, Ordering::Release);
    scope_store.advance(scope(), state(1, 1), state(2, 1)).await?;
    scope_store.advance(scope(), state(1, 1), state(2, 1)).await?;
    let mut current = owner.clone();
    current.context.observed_acl = Revision(2);
    current.holding.acl_revision = Revision(2);
    assert!(matches!(
        blocks.acquire_root(operation(100), request(current.clone(), root.clone(), 70)).await,
        Err(Error::Busy)
    ));
    let queue = SqliteScheduler::new(db.clone(), operation(246), holder(), Duration::from_secs(10))?;
    let mut interrupted = None;
    for _ in 0..128 {
        let lease = queue.due(now_millis() + 100_000, 1).await?.pop().ok_or("missing holding task")?;
        let outcome = process(&blocks, lease.clone()).await?;
        let n = moved(&db, &current.holding)?;
        if n > 0 && n < 70 {
            interrupted = Some(lease);
            break;
        }
        queue.finish(lease, outcome).await?;
    }
    let old_lease = interrupted.ok_or("maintenance did not pause between use pages")?;
    drop(queue);
    drop(scope_store);
    drop(blocks);
    drop(db);
    super::super::scheduler::closed(&path).await?;

    let db = open(&path)?;
    let blocks = store(&db, &dir, gate)?;
    let scope_store = scopes(&blocks);
    scope_store.advance(scope(), state(2, 1), state(2, 2)).await?;
    current.holding.publication_epoch = Revision(2);
    let queue = SqliteScheduler::new(db.clone(), operation(247), holder(), Duration::from_secs(10))?;
    queue.finish(old_lease.clone(), Outcome::Completed { through: old_lease.task.wanted }).await?;
    // New-period staging is independent of the old-period sweep.
    let fresh = blocks.put(current.clone(), operation(125), encoded.representation, encoded.bytes).await?;
    drain(&db, &blocks).await?;
    assert_eq!(moved(&db, &current.holding)?, 70);
    assert!(matches!(
        blocks.acquire_root(operation(100), request(current.clone(), root, 70)).await?,
        RootAcquired::Held { .. }
    ));
    db.read_now(|db| {
        let retired: bool = db.query_row(
            "SELECT released FROM sync_root_leases WHERE lease=?1",
            [RootLeaseId::from_bytes([200; 32]).as_str()],
            |row| row.get(0),
        )?;
        assert!(retired);
        let active: i64 = db.query_row(
            "SELECT COUNT(*) FROM sync_supply_checks WHERE operation=?1 AND phase IN ('walking','waiting','ready')",
            [operation(110).as_str()],
            |row| row.get(0),
        )?;
        assert_eq!(active, 0);
        let old: i64 = db.query_row(
            "SELECT COUNT(*) FROM sync_asset_pins WHERE phase!='released' AND holding!=?1",
            [serde_json::to_vec(&current.holding).unwrap()],
            |row| row.get(0),
        )?;
        assert_eq!(old, 0);
        let progress: Vec<u8> =
            db.query_row("SELECT progress FROM sync_scope_reconciliations", [], |row| row.get(0))?;
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&progress).unwrap(), serde_json::json!("Done"));
        Ok(())
    })?;
    assert!(blocks.descriptor(current.clone(), fresh.content.id).await.is_ok());
    assert!(blocks.put_manifest(owner, operation(110), page).await.is_err());
    assert!(matches!(scope_store.advance(scope(), state(1, 1), state(2, 3)).await, Err(Error::Busy)));

    println!(
        "holding lifecycle: atomic fence, bounded use migration and refusal, invalidated supply, restarted scan and newer target survival passed"
    );
    Ok(())
}
