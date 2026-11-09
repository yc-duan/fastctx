//! A mobile timeline fence protects available metadata while its tree is incomplete.
use super::*;
use fastctx_sync::{
    cursor::{Count, Sequence, StreamCursor},
    manifest::{ContentRef, ManifestChild},
    store::roots::{AcquireRoot, RootAcquired, RootStore, RootUse},
};
use fastctx_types::sync::metadata::{MetadataCodec, TimelinePayload, TimelineRoot};

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("timeline-partial-hold");
    fastctx_platform::ensure_private_dir(&dir)?;
    let owner = custody(90, "conversation.timeline");
    let missing = ManifestRoot(BlobId {
        domain: scope(),
        algorithm: DigestAlgorithm::Sha256,
        digest: Digest::of(b"missing timeline child"),
    });
    let child = ManifestChild { root: missing.clone(), first_key: "a".into(), last_key: "z".into(), entries: Count(1) };
    let through = StreamCursor {
        scope: scope(),
        stream: StreamId::from_bytes([90; 32]),
        epoch: ProducerEpoch::from_bytes([90; 32]),
        committed: Sequence(1),
    };
    let payload = codec::encode(
        scope(),
        serde_json::to_vec(&TimelinePayload::Root(Box::new(TimelineRoot {
            conversation: owner.object.clone(),
            projection: Revision(1),
            through: through.clone(),
            items: Some(child.clone()),
            lookup: Some(child),
        })))?,
        Encoding::Raw,
    )?;
    let page_json = serde_json::to_string(&ManifestPage::Metadata {
        codec: MetadataCodec::TimelineV1,
        payload: payload.representation.content.clone(),
        references: vec![ContentRef::Manifest { root: missing }],
    })?;
    let page = ManifestReply {
        root: ManifestRoot(BlobId {
            domain: scope(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(page_json.as_bytes()),
        }),
        page_json,
    };
    let path = dir.join("replica.sqlite3");
    let db = open_with(&path, |db| {
        // A snapshot needs no read grant: the committed head alone binds its root.
        db.execute("INSERT INTO sync_timeline_heads(hub,scope,conversation,projection,through_cursor,root,indexed,applied,local) VALUES(?1,?2,?3,?4,?5,?6,?4,?7,?7)",params![scope().hub.to_string(),scope().scope.as_str(),owner.object.object.as_str(),1_u64.to_be_bytes().as_slice(),serde_json::to_vec(&through).unwrap(),page.root.0.digest.as_str(),1_u64.to_be_bytes().as_slice()])?;
        Ok(())
    })?;
    let blocks =
        SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(242), 8 * 1024 * 1024)?;
    let pin = blocks.put_manifest(owner.clone(), operation(90), page.clone()).await?;
    let payload = blocks.put(owner.clone(), operation(90), payload.representation, payload.bytes).await?;
    pump(&db, &blocks).await?;
    assert!(!complete(&db, &page.root)?);
    let grants = || -> Result<i64> {
        Ok(db.read_now(|db| Ok(db.query_row("SELECT COUNT(*) FROM sync_asset_access", [], |row| row.get(0))?))?)
    };
    let authorized = grants()?;
    let request = AcquireRoot {
        lease: RootLeaseId::from_bytes([90; 32]),
        custody: owner.clone(),
        root: ContentRef::Manifest { root: page.root.clone() },
        usage: RootUse::Timeline { projection: Revision(1) },
        bases: vec![],
        expires_at_millis: Some(now_millis() + 3_600_000),
    };
    let RootAcquired::Held { lease } = blocks.acquire_root(operation(90), request.clone()).await? else {
        return Err("partial timeline was not held".into());
    };
    let mut other = request;
    other.lease = RootLeaseId::from_bytes([91; 32]);
    other.custody = custody(91, "conversation.timeline");
    assert!(blocks.acquire_root(operation(91), other).await.is_err());
    blocks.release(owner.clone(), pin).await?;
    blocks.release(owner.clone(), payload.pin).await?;
    assert_eq!(blocks.collect(128).await?, 0);
    assert_eq!(grants()?, authorized, "fencing metadata did not add body grants");
    blocks.release_root(owner, *lease).await?;
    for _ in 0..8 {
        if blocks.collect(128).await? == 0 {
            break;
        }
    }
    // The committed head still names the snapshot, so its stored page and
    // payload stay after the fence ends; superseded heads are collected in
    // the installation lifecycle.
    assert_eq!(physical_files(&dir.join("files"))?, 2);
    println!(
        "timeline usage: incomplete head root held without a grant, missing bodies remain private, the head retains it after release"
    );
    Ok(())
}
