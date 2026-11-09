//! Incremental adoption preserves private bases across GC, cancellation and restart.
use super::*;
use fastctx_sync::{
    manifest::{ContentRef, DirectoryEntry, FileEntry, PathName, RelativePath},
    store::roots::{AcquireRoot, RootAcquired, RootLease, RootStore, RootUse, SupplyBasis},
};

fn store(db: &Database, dir: &Path) -> Result<SqliteBlocks> {
    Ok(SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(241), 8 * 1024 * 1024)?)
}
fn request(custody: Custody, root: ContentRef, id: u8, basis: Option<&RootLease>) -> AcquireRoot {
    AcquireRoot {
        lease: RootLeaseId::from_bytes([id; 32]),
        custody,
        root,
        usage: RootUse::Checkpoint { source: SourceId::from_bytes([80; 32]) },
        bases: basis.into_iter().cloned().map(|lease| SupplyBasis::Held { lease: Box::new(lease) }).collect(),
        expires_at_millis: None,
    }
}
fn held(value: RootAcquired) -> Result<RootLease> {
    match value {
        RootAcquired::Held { lease } => Ok(*lease),
        _ => Err("supply check did not finish".into()),
    }
}
fn manifest(page: ManifestPage) -> Result<ManifestReply> {
    let page_json = serde_json::to_string(&page)?;
    Ok(ManifestReply {
        root: ManifestRoot(BlobId {
            domain: scope(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(page_json.as_bytes()),
        }),
        page_json,
    })
}
fn directory(
    block: &fastctx_sync::manifest::ContentBlock,
    extra: Option<&fastctx_sync::manifest::ContentBlock>,
) -> Result<ManifestReply> {
    let entries = (0..(160 + usize::from(extra.is_some())))
        .map(|index| {
            let block = if index == 160 { extra.expect("extra entry") } else { block };
            DirectoryEntry::File {
                file: FileEntry {
                    object: ObjectId::from_bytes(Digest::of(index.to_string().as_bytes()).bytes()),
                    path: RelativePath { components: vec![PathName::Utf8(format!("{index:04}"))] },
                    bytes: ByteCount(u64::from(block.decoded_bytes)),
                    executable: false,
                    content: block.clone().into(),
                },
            }
        })
        .collect();
    manifest(ManifestPage::Directory { entries })
}
fn private_holds(db: &Database) -> Result<u32> {
    Ok(db.read_now(|db| {
        Ok(db.query_row(
            "SELECT COUNT(*) FROM sync_root_leases WHERE released=0 AND supply_id IS NOT NULL",
            [],
            |row| row.get(0),
        )?)
    })?)
}
pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("supply-lifecycle");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let blocks = store(&db, &dir)?;
    let owner = custody(80, "conversation.artifact");
    let encoded = codec::encode(scope(), b"old private prefix".to_vec(), Encoding::Raw)?;
    let block = blocks.put(owner.clone(), operation(80), encoded.representation.clone(), encoded.bytes.clone()).await?;
    let old = directory(&block.content, None)?;
    let old_pin = blocks.put_manifest(owner.clone(), operation(80), old.clone()).await?;
    pump(&db, &blocks).await?;
    let acquire = request(owner.clone(), ContentRef::Manifest { root: old.root.clone() }, 80, None);
    assert!(matches!(blocks.acquire_root(operation(80), acquire.clone()).await?, RootAcquired::Pending { .. }));
    pump(&db, &blocks).await?;
    let old_hold = held(blocks.acquire_root(operation(80), acquire).await?)?;
    blocks.release(owner.clone(), old_pin).await?;
    blocks.release(owner.clone(), block.pin).await?;

    // Another operation cannot borrow a private root just by naming its hash.
    assert!(blocks.acquire_root(operation(81), request(owner.clone(), old_hold.root.clone(), 81, None)).await.is_err());
    // Supplying the parent does not supply another object's existing child.
    let foreign = custody(81, "conversation.artifact");
    let stolen = page(block.content.clone())?;
    blocks.put_manifest(foreign.clone(), operation(81), stolen.clone()).await?;
    pump(&db, &blocks).await?;
    assert!(
        blocks
            .acquire_root(operation(81), request(foreign.clone(), ContentRef::Manifest { root: stolen.root }, 81, None))
            .await
            .is_err()
    );
    assert_eq!(blocks.cancel_upload(foreign, operation(81), 128).await?, 1);
    assert_eq!(blocks.collect(128).await?, 1);
    let new_bytes = codec::encode(scope(), b"new tail".to_vec(), Encoding::Raw)?;
    let extra = blocks.put(owner.clone(), operation(82), new_bytes.representation, new_bytes.bytes).await?;
    let newer = directory(&block.content, Some(&extra.content))?;
    let new_pin = blocks.put_manifest(owner.clone(), operation(82), newer.clone()).await?;
    let next = request(owner.clone(), ContentRef::Manifest { root: newer.root.clone() }, 82, Some(&old_hold));
    assert!(matches!(blocks.acquire_root(operation(82), next.clone()).await?, RootAcquired::Pending { .. }));
    assert_eq!(private_holds(&db)?, 1);
    blocks.release_root(owner.clone(), old_hold.clone()).await?;
    assert_eq!(blocks.collect(128).await?, 0, "the verifier owns an independent basis hold");
    drop(blocks);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let db = open(&path)?;
    let blocks = store(&db, &dir)?;
    pump(&db, &blocks).await?;
    let next_hold = held(blocks.acquire_root(operation(82), next.clone()).await?)?;
    assert_eq!(private_holds(&db)?, 0);
    blocks.release(owner.clone(), new_pin).await?;
    blocks.release(owner.clone(), extra.pin).await?;
    assert_eq!(blocks.collect(128).await?, 1, "only the old manifest became unowned");
    assert!(blocks.descriptor(owner.clone(), block.content.id.clone()).await.is_err());

    // Cancellation retires verifier protections even when its original use went away.
    let cancelled_pin = blocks.put_manifest(owner.clone(), operation(83), old.clone()).await?;
    let cancelled = request(owner.clone(), ContentRef::Manifest { root: old.root.clone() }, 83, Some(&next_hold));
    assert!(matches!(blocks.acquire_root(operation(83), cancelled.clone()).await?, RootAcquired::Pending { .. }));
    blocks.release_root(owner.clone(), next_hold).await?;
    assert_eq!(blocks.cancel_upload(owner.clone(), operation(83), 128).await?, 1);
    pump(&db, &blocks).await?;
    assert_eq!(private_holds(&db)?, 0);
    assert!(blocks.acquire_root(operation(83), cancelled).await.is_err());
    assert!(blocks.put_manifest(owner.clone(), operation(83), old).await.is_err());
    blocks.release(owner.clone(), cancelled_pin).await?;
    for _ in 0..8 {
        if blocks.collect(128).await? == 0 {
            break;
        }
    }
    assert_eq!(physical_files(&dir.join("files"))?, 1);
    // Completion cannot outlive even a single supplied pin's retirement.
    let ready_block = blocks.put(owner.clone(), operation(84), encoded.representation, encoded.bytes).await?;
    let ready_page = directory(&ready_block.content, None)?;
    let ready_pin = blocks.put_manifest(owner.clone(), operation(84), ready_page.clone()).await?;
    let pending = request(owner.clone(), ContentRef::Manifest { root: ready_page.root }, 84, None);
    assert!(matches!(blocks.acquire_root(operation(84), pending.clone()).await?, RootAcquired::Pending { .. }));
    pump(&db, &blocks).await?;
    let ready_count: i64 = db.read_now(|db| {
        Ok(db.query_row(
            "SELECT COUNT(*) FROM sync_supply_checks WHERE operation=?1 AND phase='ready'",
            [operation(84).as_str()],
            |row| row.get(0),
        )?)
    })?;
    assert_eq!(ready_count, 1, "the old supply receipt really reached Ready");
    blocks.release(owner.clone(), ready_pin).await?;
    assert!(blocks.acquire_root(operation(84), pending).await.is_err());
    blocks.cancel_upload(owner, operation(84), 128).await?;
    pump(&db, &blocks).await?;
    for _ in 0..8 {
        if blocks.collect(128).await? == 0 {
            break;
        }
    }
    assert_eq!(physical_files(&dir.join("files"))?, 1);
    println!(
        "supply lifecycle: bounded restart, private operation isolation, protected basis release, GC and cancellation passed"
    );
    Ok(())
}
