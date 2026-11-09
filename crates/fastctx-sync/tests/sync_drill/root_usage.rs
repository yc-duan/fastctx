//! Durable uses outlive upload pins, but never grant general body access.
use super::*;
use fastctx_sync::{
    manifest::ContentRef,
    store::roots::{AcquireRoot, RootAcquired, RootLease, RootStore, RootUse},
};

fn request(custody: Custody, root: ContentRef, id: u8) -> AcquireRoot {
    AcquireRoot {
        lease: RootLeaseId::from_bytes([id; 32]),
        custody,
        root,
        usage: RootUse::Checkpoint { source: SourceId::from_bytes([31; 32]) },
        expires_at_millis: None,
    }
}
fn held(value: RootAcquired) -> Result<RootLease> {
    match value {
        RootAcquired::Held { lease } => Ok(*lease),
        RootAcquired::Pending { .. } => Err("complete closure was not retained".into()),
    }
}
fn store(db: &Database, dir: &Path) -> Result<SqliteBlocks> {
    Ok(SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(240), 8 * 1024 * 1024)?)
}
fn active(db: &Database) -> Result<u32> {
    Ok(db.read_now(|db| {
        Ok(db.query_row("SELECT COUNT(*) FROM sync_root_leases WHERE released=0", [], |row| row.get(0))?)
    })?)
}
pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("root-usage");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let blocks = store(&db, &dir)?;
    let owner = custody(31, "conversation.artifact");
    let raw = codec::encode(scope(), b"partial host parser state".to_vec(), Encoding::Raw)?;
    let manifest = page(raw.representation.content.clone())?;
    let upload = blocks.put_manifest(owner.clone(), operation(31), manifest.clone()).await?;
    let acquire = request(owner.clone(), ContentRef::Manifest { root: manifest.root.clone() }, 31);
    assert!(matches!(blocks.acquire_root(operation(31), acquire.clone()).await?, RootAcquired::Pending { .. }));
    assert_eq!(active(&db)?, 0);
    let bytes = blocks.put(owner.clone(), operation(31), raw.representation, raw.bytes).await?;
    pump(&db, &blocks).await?;
    let mut waiting = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let blocks = blocks.clone();
        let acquire = acquire.clone();
        waiting.spawn(async move { blocks.acquire_root(operation(31), acquire).await });
    }
    let first = held(waiting.join_next().await.ok_or("missing first acquisition")???)?;
    assert_eq!(held(waiting.join_next().await.ok_or("missing retry acquisition")???)?.id, first.id);
    assert_eq!(active(&db)?, 1);
    blocks.release(owner.clone(), upload).await?;
    blocks.release(owner.clone(), bytes.pin).await?;
    assert_eq!(blocks.collect(128).await?, 0);
    assert!(complete(&db, &manifest.root)?);
    // The private checkpoint lifetime must not become a normal object grant.
    assert!(blocks.manifest(owner.clone(), manifest.root.clone()).await.is_err());
    assert!(blocks.descriptor(owner.clone(), bytes.content.id).await.is_err());
    assert_eq!(
        db.read_now(|db| Ok(db.query_row("SELECT COUNT(*) FROM sync_asset_access", [], |row| row.get::<_, u32>(0))?))?,
        0
    );
    let mut other_source = request(owner.clone(), first.root.clone(), 39);
    other_source.usage = RootUse::Checkpoint { source: SourceId::from_bytes([39; 32]) };
    assert!(blocks.acquire_root(operation(39), other_source).await.is_err());
    let second_request = request(owner.clone(), first.root.clone(), 32);
    let second = held(blocks.acquire_root(operation(32), second_request).await?)?;
    let mut other = acquire.clone();
    other.lease = RootLeaseId::from_bytes([33; 32]);
    other.custody.object.object = ObjectId::from_bytes([33; 32]);
    assert!(blocks.acquire_root(operation(33), other).await.is_err());
    let mut forged = first.clone();
    forged.operation = operation(34);
    assert!(blocks.release_root(owner.clone(), forged).await.is_err());
    let mut wrong_kind = owner.clone();
    wrong_kind.kind = REGISTRY.asset("conversation.plan")?.registered();
    assert!(blocks.release_root(wrong_kind, first.clone()).await.is_err());
    drop(blocks);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let db = open(&path)?;
    let blocks = store(&db, &dir)?;
    assert_eq!(held(blocks.acquire_root(operation(31), acquire.clone()).await?)?.id, first.id);
    blocks.release_root(owner.clone(), first.clone()).await?;
    blocks.release_root(owner.clone(), first).await?;
    assert!(blocks.acquire_root(operation(31), acquire).await.is_err());
    assert_eq!(blocks.collect(128).await?, 0);
    blocks.release_root(owner.clone(), second).await?;
    assert_eq!(blocks.collect(128).await?, 1);
    assert_eq!(blocks.collect(128).await?, 1);
    assert_eq!(active(&db)?, 0);

    // Expiry belongs to the local use, never to any independently retained asset.
    let raw = codec::encode(scope(), b"runtime staging with a finite use".to_vec(), Encoding::Raw)?;
    let published = blocks.put(owner.clone(), operation(35), raw.representation, raw.bytes).await?;
    let mut usage = request(owner.clone(), published.content.clone().into(), 35);
    usage.usage = RootUse::Runtime { kind: "runtime.transport".into(), record_key: "staged-output".into() };
    usage.expires_at_millis = Some(now_millis() + 3_600_000);
    held(blocks.acquire_root(operation(35), usage).await?)?;
    blocks.release(owner.clone(), published.pin).await?;
    let raw = codec::encode(scope(), b"checkpoint surviving until scope revocation".to_vec(), Encoding::Raw)?;
    let published = blocks.put(owner.clone(), operation(36), raw.representation, raw.bytes).await?;
    let retained = request(owner.clone(), published.content.clone().into(), 36);
    let retained = held(blocks.acquire_root(operation(36), retained).await?)?;
    blocks.release(owner.clone(), published.pin).await?;
    drop(blocks);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let db = open_with(&path, |db| {
        db.execute("UPDATE sync_root_leases SET expires=0 WHERE expires IS NOT NULL", [])?;
        Ok(())
    })?;
    let blocks = store(&db, &dir)?;
    assert_eq!(blocks.collect(1).await? + blocks.collect(1).await?, 1);
    assert_eq!(active(&db)?, 1);
    drop(blocks);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let db = open_with(&path, |db| {
        db.execute("UPDATE sync_scopes SET enabled=0", [])?;
        Ok(())
    })?;
    let blocks = store(&db, &dir)?;
    let mut stranger = owner.clone();
    stranger.holding.holder = Holder::Device { device: DeviceId::from_bytes([99; 32]) };
    assert!(blocks.release_root(stranger, retained.clone()).await.is_err());
    assert_eq!(blocks.retire_root_uses(scope(), 1).await?, 1);
    blocks.release_root(owner.clone(), retained.clone()).await?;
    blocks.release_root(owner, retained).await?;
    assert_eq!(blocks.retire_root_uses(scope(), 1).await?, 0);
    assert_eq!(blocks.collect(1).await?, 1);
    assert_eq!(active(&db)?, 0);
    advanced_holding(root).await?;
    private_supply(root).await?;
    println!(
        "root usage: pending closure, concurrent retry, restart, private custody, independent uses, expiry, old-period release and revocation passed"
    );
    Ok(())
}

async fn advanced_holding(root: &Path) -> Result<()> {
    let dir = root.join("advanced-root-holding");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let blocks = store(&db, &dir)?;
    let old = custody(50, "conversation.artifact");
    let content = codec::encode(scope(), b"state retained across an ACL advance".to_vec(), Encoding::Raw)?;
    let published = blocks.put(old.clone(), operation(50), content.representation, content.bytes).await?;
    let root_request = request(old.clone(), published.content.into(), 50);
    let lease = held(blocks.acquire_root(operation(50), root_request.clone()).await?)?;
    drop(blocks);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let mut current = old.clone();
    current.holding.acl_revision = Revision(2);
    current.holding.publication_epoch = Revision(2);
    current.context.observed_acl = Revision(2);
    let period = serde_json::to_vec(&current.holding)?;
    let db = open_with(&path, |db| {
        db.execute("UPDATE sync_scopes SET acl_revision=?1,publication_epoch=?1", [2_u64.to_be_bytes().as_slice()])?;
        db.execute("UPDATE sync_root_leases SET holding=?1", [&period])?;
        Ok(())
    })?;
    let blocks = store(&db, &dir)?;
    assert!(blocks.acquire_root(operation(50), root_request).await.is_err());
    assert_eq!(blocks.cancel_upload(current.clone(), operation(50), 8).await?, 1);
    assert_eq!(blocks.cancel_upload(old.clone(), operation(50), 8).await?, 0);
    assert_eq!(blocks.collect(8).await?, 0);
    blocks.release_root(current, lease.clone()).await?;
    blocks.release_root(old, lease).await?;
    assert_eq!(blocks.collect(8).await?, 1);
    Ok(())
}

async fn private_supply(root: &Path) -> Result<()> {
    let dir = root.join("private-source-staging");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let blocks = store(&db, &dir)?;
    let reader = custody(60, "conversation.artifact");
    let mut source = reader.clone();
    source.purpose = HoldingUse::WorkspaceSource {
        workspace: fastctx_sync::store::access::WorkspaceBinding {
            conversation: source.object.clone(),
            account: AccountId::from_bytes([61; 32]),
            machine: DeviceId::from_bytes([3; 32]),
            epoch: WorkspaceEpoch::from_bytes([62; 32]),
            registration: operation(60),
        },
    };
    let content =
        codec::encode(scope(), b"private parser state before committing a checkpoint".to_vec(), Encoding::Raw)?;
    let manifest = page(content.representation.content.clone())?;
    let published =
        blocks.put(source.clone(), operation(60), content.representation.clone(), content.bytes.clone()).await?;
    blocks.put_manifest(source.clone(), operation(60), manifest.clone()).await?;
    pump(&db, &blocks).await?;
    assert!(blocks.descriptor(reader.clone(), published.content.id.clone()).await.is_err());
    assert!(blocks.read(reader.clone(), published.content.id.clone(), published.representation.clone()).await.is_err());
    assert!(blocks.manifest(reader.clone(), manifest.root.clone()).await.is_err());
    assert!(blocks.read(source.clone(), published.content.id.clone(), published.representation).await.is_err());
    let usage = held(
        blocks
            .acquire_root(
                operation(60),
                request(source.clone(), ContentRef::Manifest { root: manifest.root.clone() }, 60),
            )
            .await?,
    )?;
    assert_eq!(blocks.cancel_upload(source.clone(), operation(60), 8).await?, 2);
    assert!(blocks.manifest(reader.clone(), manifest.root).await.is_err());
    assert_eq!(blocks.collect(8).await?, 0);
    // Supplying the actual bytes for Replica use creates a distinct public pin.
    let public = blocks.put(reader.clone(), operation(61), content.representation, content.bytes.clone()).await?;
    assert_eq!(blocks.read(reader.clone(), public.content.id, public.representation).await?, content.bytes);
    blocks.release(reader, public.pin).await?;
    blocks.release_root(source, usage).await?;
    assert_eq!(blocks.collect(8).await? + blocks.collect(8).await?, 2);
    Ok(())
}
