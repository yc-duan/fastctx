//! Real content files, writer transactions and collection share one lifecycle.
#[path = "canonical_producer.rs"]
mod canonical_producer;
#[path = "directory.rs"]
mod directory;
#[path = "holding.rs"]
mod holding;
#[path = "metadata.rs"]
mod metadata;
#[path = "pending.rs"]
mod pending;
#[path = "block_recovery.rs"]
mod recovery;
#[path = "replica.rs"]
pub(crate) mod replica;
#[path = "root_usage.rs"]
mod root_usage;
#[path = "source_acceptance.rs"]
mod source_acceptance;
#[path = "source_confirmation.rs"]
mod source_confirmation;
#[path = "source_ingest.rs"]
mod source_ingest;
#[path = "source_steps.rs"]
mod source_steps;
#[path = "supply.rs"]
mod supply;
#[path = "timeline_hold.rs"]
mod timeline_hold;
#[path = "timeline_install.rs"]
mod timeline_install;
use fastctx_storage::{Database, rusqlite::params};
use fastctx_sync::{
    blocks::{SqliteBlocks, codec},
    cursor::{ByteCount, Holder, HoldingPeriod, Revision},
    identity::*,
    manifest::{BlobId, BlockStore, DigestAlgorithm, Encoding, ManifestPage, ManifestReply, ManifestRoot},
    registry::{REGISTRY, RegisteredKind},
    scheduler::{Scheduler, SqliteScheduler},
    store::{
        access::{AccessContext, AccessPolicy, Custody, Decision, HoldingUse, Principal},
        boundary, schema,
    },
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Policy {
    scope: ScopeKey,
    extra: Option<ScopeKey>,
    enabled: AtomicBool,
    checks: AtomicUsize,
    first_only: bool,
}
impl Policy {
    fn check(&self, context: &AccessContext, kind: RegisteredKind) -> Decision {
        if self.enabled.load(Ordering::Acquire)
            && (context.scope == self.scope || self.extra.as_ref() == Some(&context.scope))
            && context.observed_acl == Revision(1)
            && matches!(kind.registration().id, "conversation.plan" | "conversation.artifact" | "conversation.timeline")
        {
            Decision::Allowed { revision: Revision(1) }
        } else {
            Decision::Denied { revision: Revision(1), reason: "outside the drill's scope/kind policy".into() }
        }
    }
}
impl AccessPolicy for Policy {
    fn can_read(&self, c: &AccessContext, k: RegisteredKind) -> Decision {
        self.check(c, k)
    }
    fn can_read_object(&self, c: &AccessContext, k: RegisteredKind, object: &ObjectKey) -> Decision {
        if object.scope != c.scope {
            return Decision::Unavailable;
        }
        self.check(c, k)
    }
    fn can_write(&self, c: &AccessContext, k: RegisteredKind) -> Decision {
        self.check(c, k)
    }
    fn can_keep(&self, c: &AccessContext, k: RegisteredKind, o: &ObjectKey, _: &HoldingUse) -> Decision {
        if o.scope != c.scope || self.first_only && self.checks.fetch_add(1, Ordering::AcqRel) != 0 {
            Decision::Denied { revision: Revision(1), reason: "publication lost custody".into() }
        } else {
            self.check(c, k)
        }
    }
    fn can_execute(&self, _: &AccessContext, _: &DeviceId) -> Decision {
        Decision::Denied { revision: Revision(1), reason: "content lifecycle has no execution grants".into() }
    }
}
fn operation(n: u8) -> OperationId {
    OperationId::from_bytes([n; 32])
}
fn scope() -> ScopeKey {
    ScopeKey { hub: HubId::from_bytes([1; 8]), scope: ScopeId::from_bytes([2; 32]) }
}
fn holder() -> Holder {
    Holder::Device { device: DeviceId::from_bytes([3; 32]) }
}
fn custody(object: u8, kind: &str) -> Custody {
    let scope = scope();
    Custody {
        context: AccessContext { principal: Principal::LocalService, scope: scope.clone(), observed_acl: Revision(1) },
        object: ObjectKey { scope: scope.clone(), object: ObjectId::from_bytes([object; 32]) },
        kind: REGISTRY.asset(kind).unwrap().registered(),
        purpose: HoldingUse::Replica,
        holding: HoldingPeriod { holder: holder(), scope, acl_revision: Revision(1), publication_epoch: Revision(1) },
    }
}
fn open(path: &Path) -> Result<Database> {
    open_with(path, |_| Ok(()))
}
fn open_with(
    path: &Path,
    initialize: impl Fn(&fastctx_storage::rusqlite::Connection) -> fastctx_storage::Result<()>,
) -> Result<Database> {
    Ok(Database::open_guarded(path, "", boundary::guard, |db| {
        schema::initialize(db)?;
        db.execute("INSERT INTO sync_scopes(hub,scope,origin,acl_revision,enabled,publication_epoch) VALUES(?1,?2,X'7b7d',?3,1,?3) ON CONFLICT(hub,scope) DO NOTHING",params![scope().hub.to_string(),scope().scope.as_str(),1_u64.to_be_bytes().as_slice()])?;
        initialize(db)
    })?)
}
fn policy(first_only: bool) -> Arc<Policy> {
    Arc::new(Policy {
        scope: scope(),
        extra: None,
        enabled: AtomicBool::new(true),
        checks: AtomicUsize::new(0),
        first_only,
    })
}
fn page(block: fastctx_sync::manifest::ContentBlock) -> Result<ManifestReply> {
    let bytes = serde_json::to_string(&ManifestPage::Blocks { offset: ByteCount(0), blocks: vec![block] })?;
    Ok(ManifestReply {
        root: ManifestRoot(BlobId {
            domain: scope(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(bytes.as_bytes()),
        }),
        page_json: bytes,
    })
}
async fn pump(db: &Database, store: &SqliteBlocks) -> Result<()> {
    let scheduler = SqliteScheduler::new(db.clone(), operation(220), holder(), Duration::from_secs(10))?;
    for _ in 0..256 {
        let leases = scheduler.due(now_millis(), 16).await?;
        if leases.is_empty() {
            return Ok(());
        }
        for lease in leases {
            let outcome = store.process(lease.clone()).await?;
            scheduler.finish(lease, outcome).await?;
        }
    }
    Err("content work did not drain in its bounded scenario".into())
}
fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}
fn complete(db: &Database, root: &ManifestRoot) -> Result<bool> {
    let hash = root.0.digest.as_str();
    Ok(db.read_now(|db| {
        Ok(db.query_row(
            "SELECT complete FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![scope().hub.to_string(), scope().scope.as_str(), hash],
            |row| row.get(0),
        )?)
    })?)
}

fn physical_files(root: &Path) -> Result<usize> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = 0;
    while let Some(directory) = pending.pop() {
        for entry in fastctx_platform::disk::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                files += 1;
            }
        }
    }
    Ok(files)
}

pub async fn run(root: &Path) -> Result<()> {
    // The upstream ZSTD_estimateDStreamSize bound includes the context, window,
    // one input block and two output blocks, plus wild-copy padding.
    let decoder_state = zstd::zstd_safe::DCtx::create().sizeof();
    let decoder_bound = decoder_state + 1024 * 1024 + 3 * zstd::zstd_safe::DCtx::out_size() + 64 + 64 * 1024;
    assert!(decoder_bound <= codec::decode_workspace(Encoding::Zstd));
    let dir = root.join("content");
    fastctx_platform::ensure_private_dir(&dir)?;
    let database = open(&dir.join("replica.sqlite3"))?;
    let gate = policy(false);
    let store = SqliteBlocks::new(
        database.clone(),
        dir.join("files"),
        holder(),
        gate.clone(),
        operation(200),
        8 * 1024 * 1024,
    )?;
    assert!(
        SqliteBlocks::new(database.clone(), dir.join("files"), holder(), gate.clone(), operation(201), 8 * 1024 * 1024)
            .is_err()
    );
    let a = custody(10, "conversation.plan");
    let b = custody(11, "conversation.artifact");
    let body = (0..1024 * 1024).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let raw = codec::encode(scope(), body.clone(), Encoding::Raw)?;
    let manifest = page(raw.representation.content.clone())?;
    let manifest_pin = store.put_manifest(a.clone(), operation(1), manifest.clone()).await?;
    assert!(!complete(&database, &manifest.root)?);
    let raw_pin = store.put(a.clone(), operation(2), raw.representation.clone(), raw.bytes).await?;
    let compressed = codec::encode(scope(), body.clone(), Encoding::Zstd)?;
    let compressed_bytes = compressed.bytes.clone();
    let compressed_pin =
        store.put(a.clone(), operation(3), compressed.representation.clone(), compressed.bytes).await?;
    assert_eq!(raw_pin.content, compressed_pin.content);
    let mut alternate = compressed_bytes;
    alternate.extend(zstd::stream::encode_all(&b""[..], 1)?);
    let mut descriptor = compressed.representation.clone();
    descriptor.encoded_bytes = alternate.len() as u32;
    descriptor.encoded_digest = Digest::of(&alternate);
    assert_ne!(descriptor.id(), compressed_pin.representation);
    let reused = store.put(a.clone(), operation(4), descriptor, alternate).await?;
    assert_eq!(reused.representation, compressed_pin.representation);
    assert_eq!(store.descriptor(a.clone(), raw_pin.content.id.clone()).await?.representations.len(), 2);
    assert_eq!(store.read(a.clone(), raw_pin.content.id.clone(), raw_pin.representation.clone()).await?, body);
    pump(&database, &store).await?;
    assert!(complete(&database, &manifest.root)?);

    // Merely supplying a manifest that names somebody else's block is not custody.
    let b_manifest = store.put_manifest(b.clone(), operation(5), manifest.clone()).await?;
    assert!(store.descriptor(b.clone(), raw_pin.content.id.clone()).await.is_err());
    let mut wrong_kind = a.clone();
    wrong_kind.kind = REGISTRY.asset("conversation.artifact")?.registered();
    assert!(store.release(wrong_kind, raw_pin.pin.clone()).await.is_err());
    let mut wrong_object = a.clone();
    wrong_object.object.object = ObjectId::from_bytes([12; 32]);
    assert!(store.release(wrong_object, raw_pin.pin.clone()).await.is_err());
    let mut stale = a.clone();
    stale.holding.publication_epoch = Revision(2);
    assert!(store.descriptor(stale, raw_pin.content.id.clone()).await.is_err());

    // The same JSON stored as a normal file must not retain a manifest's children.
    let json_block = codec::encode(scope(), manifest.page_json.as_bytes().to_vec(), Encoding::Raw)?;
    assert_eq!(json_block.representation.content.id, manifest.root.0);
    let json_pin = store.put(b.clone(), operation(6), json_block.representation, json_block.bytes).await?;
    for pin in [raw_pin.pin.clone(), compressed_pin.pin, reused.pin, manifest_pin] {
        store.release(a.clone(), pin).await?;
    }
    store.release(b.clone(), b_manifest).await?;
    for _ in 0..8 {
        if store.collect(128).await? == 0 {
            break;
        }
    }
    assert!(store.descriptor(a.clone(), raw_pin.content.id.clone()).await.is_err());
    assert!(store.manifest(b.clone(), manifest.root.clone()).await.is_err());
    assert_eq!(
        database.read_now(|db| Ok(db.query_row(
            "SELECT COUNT(*) FROM sync_asset_blocks WHERE hash=?1",
            [raw_pin.content.id.digest.as_str()],
            |row| row.get::<_, i64>(0)
        )?))?,
        0
    );
    assert_eq!(physical_files(&dir.join("files"))?, 2);
    assert_eq!(
        store.read(b.clone(), json_pin.content.id.clone(), json_pin.representation.clone()).await?,
        manifest.page_json.as_bytes()
    );
    store.release(b.clone(), json_pin.pin.clone()).await?;
    store.release(b.clone(), json_pin.pin).await?;
    assert!(
        store
            .put(
                b.clone(),
                operation(6),
                codec::encode(scope(), manifest.page_json.as_bytes().to_vec(), Encoding::Raw)?.representation,
                manifest.page_json.as_bytes().to_vec()
            )
            .await
            .is_err()
    );
    store.collect(128).await?;

    // A deterministic filesystem fault leaves only hidden metadata, never a receipt.
    let failed = codec::encode(scope(), b"a disk operation that cannot publish".to_vec(), Encoding::Raw)?;
    let parent = dir.join("files").join(scope().hub.to_string()).join(scope().scope.as_str());
    fastctx_platform::ensure_private_dir(&parent)?;
    let obstruction = parent.join(&failed.representation.content.id.digest.as_str()[..2]);
    assert!(!obstruction.exists());
    fastctx_platform::disk::write(&obstruction, b"not a directory")?;
    assert!(store.put(a.clone(), operation(7), failed.representation.clone(), failed.bytes.clone()).await.is_err());
    assert!(store.descriptor(a.clone(), failed.representation.content.id.clone()).await.is_err());
    fastctx_platform::disk::remove_file(&obstruction)?;
    let recovered = store.put(a.clone(), operation(7), failed.representation, failed.bytes).await?;
    assert_eq!(
        store.read(a.clone(), recovered.content.id.clone(), recovered.representation.clone()).await?,
        b"a disk operation that cannot publish"
    );
    gate.enabled.store(false, Ordering::Release);
    assert!(store.read(a.clone(), recovered.content.id.clone(), recovered.representation.clone()).await.is_err());
    gate.enabled.store(true, Ordering::Release);
    assert_eq!(
        store.read(a.clone(), recovered.content.id.clone(), recovered.representation).await?,
        b"a disk operation that cannot publish"
    );
    assert!(store.retire_uploads(scope(), 1).await.is_err());
    assert_eq!(store.cancel_upload(a.clone(), operation(7), 1).await?, 1);
    assert_eq!(store.cancel_upload(a.clone(), operation(7), 1).await?, 0);
    let cancelled = codec::encode(scope(), b"a disk operation that cannot publish".to_vec(), Encoding::Raw)?;
    assert!(store.put(a.clone(), operation(7), cancelled.representation, cancelled.bytes).await.is_err());
    assert_eq!(store.collect(128).await?, 1);
    assert_eq!(store.cancel_upload(a.clone(), operation(9), 1).await?, 0);
    let late = codec::encode(scope(), b"arriving after cancellation".to_vec(), Encoding::Raw)?;
    assert!(store.put(a.clone(), operation(9), late.representation, late.bytes).await.is_err());

    assert!(
        database
            .write(1024, |db| {
                db.execute("UPDATE sync_asset_blocks SET verified=1", [])?;
                Ok(())
            })
            .await
            .is_err()
    );
    drop(store);
    drop(database);

    let raced = dir.join("revoked-publication");
    fastctx_platform::ensure_private_dir(&raced)?;
    let db = open(&raced.join("replica.sqlite3"))?;
    let racing =
        SqliteBlocks::new(db.clone(), raced.join("files"), holder(), policy(true), operation(202), 8 * 1024 * 1024)?;
    let bytes = codec::encode(scope(), b"not acknowledged after custody changed".to_vec(), Encoding::Raw)?;
    assert!(racing.put(a, operation(8), bytes.representation, bytes.bytes).await.is_err());
    assert_eq!(
        db.read_now(|db| Ok(
            db.query_row("SELECT COUNT(*) FROM sync_asset_blocks WHERE verified=1", [], |row| row.get::<_, i64>(0))?
        ))?,
        0
    );
    assert_eq!(
        db.read_now(|db| Ok(db.query_row(
            "SELECT COUNT(*) FROM sync_asset_pins WHERE phase='staging'",
            [],
            |row| row.get::<_, i64>(0)
        )?))?,
        1
    );
    drop(racing);
    drop(db);
    super::scheduler::closed(&raced.join("replica.sqlite3")).await?;
    let db = open_with(&raced.join("replica.sqlite3"), |db| {
        db.execute("UPDATE sync_scopes SET enabled=0", [])?;
        Ok(())
    })?;
    let cleanup =
        SqliteBlocks::new(db.clone(), raced.join("files"), holder(), policy(true), operation(203), 8 * 1024 * 1024)?;
    assert_eq!(cleanup.retire_uploads(scope(), 1).await?, 1);
    assert_eq!(cleanup.retire_uploads(scope(), 1).await?, 0);
    assert_eq!(cleanup.collect(128).await?, 1);
    assert_eq!(physical_files(&raced.join("files"))?, 1);
    assert_eq!(
        db.read_now(|db| Ok(
            db.query_row("SELECT COUNT(*) FROM sync_asset_representations", [], |r| r.get::<_, i64>(0))?
        ))?,
        0
    );
    metadata::run(root).await?;
    recovery::run(root).await?;
    root_usage::run(root).await?;
    source_steps::run(root).await?;
    source_ingest::run(root).await?;
    source_confirmation::run(root).await?;
    source_acceptance::run(root).await?;
    timeline_install::run(root).await?;
    replica::run(root).await?;
    let canonical_root = root.to_owned();
    tokio::spawn(async move { canonical_producer::run(&canonical_root).await.map_err(|error| error.to_string()) })
        .await??;
    pending::run(root).await?;
    supply::run(root).await?;
    timeline_hold::run(root).await?;
    holding::run(root).await?;
    directory::run(root).await?;
    println!(
        "content lifecycle: typed pins, dual encodings, pending closures, private references, claim fences, cancellation, restart and revoked staging cleanup passed"
    );
    Ok(())
}
