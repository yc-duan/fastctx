//! Reopening a claimed generation exercises its writer and scheduler fences.
use super::*;
use fastctx_sync::{cursor::Count, manifest::ManifestChild, store::Error};

fn manifest(value: ManifestPage) -> Result<ManifestReply> {
    let page_json = serde_json::to_string(&value)?;
    Ok(ManifestReply {
        root: ManifestRoot(BlobId {
            domain: scope(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(page_json.as_bytes()),
        }),
        page_json,
    })
}

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("claimed-content");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let store =
        SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(210), 8 * 1024 * 1024)?;
    let custody = custody(20, "conversation.plan");
    let child = manifest(ManifestPage::Directory { entries: vec![] })?;
    let pin = store.put_manifest(custody.clone(), operation(20), child.clone()).await?;
    assert!(complete(&db, &child.root)?);
    store.release(custody.clone(), pin).await?;
    drop(store);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    // Recreate the durable boundary of an interrupted collection, before deletion.
    let db = open_with(&path, |db| {
        db.execute("UPDATE sync_asset_gc SET claimed=1", [])?;
        db.execute("UPDATE sync_asset_manifests SET complete=0", [])?;
        Ok(())
    })?;
    let store =
        SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(211), 8 * 1024 * 1024)?;
    pump(&db, &store).await?;
    assert!(!complete(&db, &child.root)?, "stale completion work revived a claimed page");
    let parent = manifest(ManifestPage::Branch {
        children: vec![ManifestChild {
            first_key: "a".into(),
            last_key: "z".into(),
            root: child.root.clone(),
            entries: Count(0),
        }],
    })?;
    assert!(matches!(store.put_manifest(custody.clone(), operation(21), parent.clone()).await, Err(Error::Busy)));
    assert_eq!(
        db.read_now(|db| Ok(db.query_row("SELECT COUNT(*) FROM sync_asset_edges", [], |r| r.get::<_, i64>(0))?))?,
        0
    );
    assert_eq!(store.collect(128).await?, 1);
    store.put_manifest(custody.clone(), operation(21), parent.clone()).await?;
    assert!(!complete(&db, &parent.root)?);
    store.put_manifest(custody, operation(22), child).await?;
    pump(&db, &store).await?;
    assert!(complete(&db, &parent.root)?);
    interrupted_gc(root).await?;
    Ok(())
}

async fn interrupted_gc(root: &Path) -> Result<()> {
    let dir = root.join("gc-failures");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let store =
        SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(212), 8 * 1024 * 1024)?;
    let owner = custody(40, "conversation.artifact");
    let bytes = codec::encode(scope(), file_body(b"reserved but no bucket was ever created"), Encoding::Raw)?;
    let obstruction = dir.join("files").join(scope().hub.to_string());
    fastctx_platform::disk::write(&obstruction, b"prevent namespace preparation")?;
    assert!(store.put(owner.clone(), operation(40), bytes.representation.clone(), bytes.bytes.clone()).await.is_err());
    fastctx_platform::disk::remove_file(&obstruction)?;
    assert_eq!(store.cancel_upload(owner.clone(), operation(40), 8).await?, 1);
    assert_eq!(store.collect(8).await?, 1, "an absent generation and absent buckets must retire");
    let published = store.put(owner.clone(), operation(41), bytes.representation, bytes.bytes).await?;
    store.release(owner.clone(), published.pin).await?;
    assert_eq!(store.collect(8).await?, 1, "the previous claim must not block new publication");

    let mut blocks = vec![
        codec::encode(scope(), file_body(b"one uncollectable generation"), Encoding::Raw)?,
        codec::encode(scope(), file_body(b"one independently collectable generation"), Encoding::Raw)?,
    ];
    blocks.sort_by(|a, b| a.representation.content.id.digest.cmp(&b.representation.content.id.digest));
    let bad_hash = blocks[0].representation.content.id.digest.clone();
    for (index, block) in blocks.into_iter().enumerate() {
        let pin = store.put(owner.clone(), operation(42 + index as u8), block.representation, block.bytes).await?;
        store.release(owner.clone(), pin.pin).await?;
    }
    let bad_path = dir
        .join("files")
        .join(scope().hub.to_string())
        .join(scope().scope.as_str())
        .join(&bad_hash.as_str()[..2])
        .join(&bad_hash.as_str()[2..4])
        .join(format!("{}.raw.{}", bad_hash.as_str(), operation(42).as_str()));
    fastctx_platform::disk::remove_file(&bad_path)?;
    fastctx_platform::ensure_private_dir(&bad_path)?;
    assert_eq!(store.collect(8).await?, 1, "the failed first candidate must not block the next candidate");
    let (claimed, failures, due) = db.read_now(|db| {
        Ok(db.query_row(
            "SELECT claimed,failures,next_due FROM sync_asset_gc WHERE hash=?1",
            [bad_hash.as_str()],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, u32>(1)?, row.get::<_, i64>(2)?)),
        )?)
    })?;
    assert!(claimed && failures == 1 && due > 0);
    fastctx_platform::disk::remove_dir(&bad_path)?;
    drop(store);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let db = open_with(&path, |db| {
        db.execute("UPDATE sync_asset_gc SET next_due=0", [])?;
        Ok(())
    })?;
    let store =
        SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(213), 8 * 1024 * 1024)?;
    assert_eq!(store.collect(8).await?, 1);
    assert_eq!(
        db.read_now(|db| Ok(db.query_row("SELECT COUNT(*) FROM sync_asset_gc", [], |row| row.get::<_, u32>(0))?))?,
        0
    );
    println!("GC lifecycle: missing buckets, failed candidate isolation, durable retry and claim release passed");
    Ok(())
}
