//! Index publication preserves object custody and separates metadata from bodies.
use super::*;
use fastctx_types::sync::{
    catalog::{Authority, HeadEvidence, Lifecycle, ObjectHead},
    cursor::{Completeness, ScopeCursor, Sequence, VersionRef},
    manifest::ContentRef,
    metadata::{CatalogLeaf, MetadataCodec, ProofReference},
};

fn manifest(payload: fastctx_sync::manifest::ContentBlock, references: Vec<ContentRef>) -> Result<ManifestReply> {
    let page_json =
        serde_json::to_string(&ManifestPage::Metadata { codec: MetadataCodec::CatalogV1, payload, references })?;
    Ok(ManifestReply {
        root: ManifestRoot(BlobId {
            domain: scope(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(page_json.as_bytes()),
        }),
        page_json,
    })
}

pub async fn run(root: &Path) -> Result<()> {
    let dir = root.join("indexes");
    fastctx_platform::ensure_private_dir(&dir)?;
    let db = open(&dir.join("replica.sqlite3"))?;
    let store =
        SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(204), 8 * 1024 * 1024)?;
    let index = custody(40, "conversation.artifact");
    let owner = custody(41, "conversation.plan");
    let bytes = codec::encode(scope(), b"body owned by the described object".to_vec(), Encoding::Raw)?;
    let body = store.put(owner.clone(), operation(30), bytes.representation, bytes.bytes).await?;
    // Structural publication is separate from signature verification on installation.
    let proof_bytes = codec::encode(scope(), b"invalid signature: structural fixture only".to_vec(), Encoding::Raw)?;
    let proof = store.put(index.clone(), operation(31), proof_bytes.representation, proof_bytes.bytes).await?;
    let proof_id = Digest::from_bytes([42; 32]);
    let version = VersionRef { authority_epoch: AuthorityEpoch::from_bytes([43; 16]), revision: Revision(1) };
    let payload = CatalogLeaf {
        cursor: ScopeCursor {
            scope: scope(),
            epoch: FeedEpoch::from_bytes([44; 32]),
            acl_revision: Revision(1),
            view: Some(ViewId::from_bytes([45; 32])),
            through: Sequence(1),
            prefix_digest: Some(Digest::from_bytes([46; 32])),
        },
        objects: vec![ObjectHead {
            key: owner.object.clone(),
            kind: "conversation.plan".into(),
            schema: 1,
            acl_revision: Revision(1),
            authority: Authority::Hub,
            evidence: HeadEvidence::Accepted { proof: proof_id.clone() },
            revision: version.clone(),
            current_version: version,
            content: Some(ContentRef::from(body.content.clone())),
            history: None,
            lifecycle: Lifecycle::Live,
            completeness: Completeness::MetadataOnly,
        }],
        proofs: vec![ProofReference { id: proof_id, payload: proof.content.clone() }],
    };
    let encoded = codec::encode(scope(), serde_json::to_vec(&payload)?, Encoding::Raw)?;
    let page = manifest(encoded.representation.content.clone(), vec![ContentRef::from(proof.content.clone())])?;
    let page_pin = store.put_manifest(index.clone(), operation(32), page.clone()).await?;
    pump(&db, &store).await?;
    assert!(!complete(&db, &page.root)?);
    let foreign =
        store.put(owner.clone(), operation(33), encoded.representation.clone(), encoded.bytes.clone()).await?;
    pump(&db, &store).await?;
    assert!(!complete(&db, &page.root)?);
    let supplied =
        store.put(index.clone(), operation(34), encoded.representation.clone(), encoded.bytes.clone()).await?;
    pump(&db, &store).await?;
    assert!(complete(&db, &page.root)?);
    assert!(store.descriptor(index.clone(), body.content.id.clone()).await.is_err());
    store.release(owner.clone(), body.pin).await?;
    store.collect(128).await?;
    assert!(complete(&db, &page.root)?);
    assert!(store.descriptor(owner.clone(), body.content.id).await.is_err());

    // Extra declared references never become trusted dependencies of this codec.
    let wrong = manifest(encoded.representation.content, vec![])?;
    let wrong_pin = store.put_manifest(index.clone(), operation(35), wrong.clone()).await?;
    let scheduler = SqliteScheduler::new(db.clone(), operation(205), holder(), Duration::from_secs(10))?;
    let mut rejected = false;
    for _ in 0..16 {
        let leases = scheduler.due(now_millis(), 16).await?;
        if leases.is_empty() {
            break;
        }
        for lease in leases {
            let outcome = match store.process(lease.clone()).await {
                Err(fastctx_sync::store::Error::Invalid(reason)) => {
                    rejected = true;
                    fastctx_sync::scheduler::Outcome::Failed { reason }
                }
                result => result?,
            };
            scheduler.finish(lease, outcome).await?;
        }
    }
    assert!(rejected);
    assert!(!complete(&db, &wrong.root)?);
    for pin in [proof.pin, supplied.pin, page_pin, wrong_pin] {
        store.release(index.clone(), pin).await?;
    }
    store.release(owner, foreign.pin).await?;
    for _ in 0..8 {
        if store.collect(128).await? == 0 {
            break;
        }
    }
    assert_eq!(physical_files(&dir.join("files"))?, 1);
    println!(
        "index lifecycle: bounded codec validation, missing/foreign payloads, reused publication wake, catalog-only retention and invalid references passed"
    );
    Ok(())
}
