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
    Ok(())
}
