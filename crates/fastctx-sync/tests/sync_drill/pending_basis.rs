//! Accepted bases and queued edits cannot be replaced with caller-invented inputs.
use super::*;
use fastctx_types::sync::{
    catalog::{Authority, HeadEvidence},
    cursor::{Completeness, VersionRef},
    mutation::TextEdit,
};

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("pending-basis");
    fastctx_platform::ensure_private_dir(&dir)?;
    let a = custody(60, "conversation.plan");
    let original = codec::encode(scope(), b"protected=no\r\nlabel=old\r\n".to_vec(), Encoding::Raw)?;
    let base = ContentRef::from(original.representation.content.clone());
    let version = VersionRef { authority_epoch: AuthorityEpoch::from_bytes([60; 16]), revision: Revision(1) };
    let db = open_with(&dir.join("replica.sqlite3"), |db| {
        db.execute("INSERT INTO sync_objects(hub,scope,object,kind,schema,authority,evidence,authority_epoch,revision,current_authority_epoch,current_version,content,history,state,completeness) VALUES(?1,?2,?3,'conversation.plan',1,?4,?5,?6,?7,?6,?7,?8,NULL,'live',?9)",params![scope().hub.to_string(),scope().scope.as_str(),a.object.object.as_str(),serde_json::to_vec(&Authority::Hub).unwrap(),serde_json::to_vec(&HeadEvidence::Accepted{proof:Digest::from_bytes([0;32])}).unwrap(),version.authority_epoch.bytes().as_slice(),version.revision.0.to_be_bytes().as_slice(),serde_json::to_vec(&base).unwrap(),serde_json::to_vec(&Completeness::MetadataOnly).unwrap()])?;
        db.execute("INSERT INTO sync_object_versions(hub,scope,object,authority_epoch,revision,actor,operation,content,parents,state) VALUES(?1,?2,?3,?4,?5,'fixture',?6,?7,X'5b5d','live')",params![scope().hub.to_string(),scope().scope.as_str(),a.object.object.as_str(),version.authority_epoch.bytes().as_slice(),version.revision.0.to_be_bytes().as_slice(),operation(60).as_str(),serde_json::to_vec(&base).unwrap()])?;
        Ok(())
    })?;
    let gate = policy(false);
    let blocks = Arc::new(SqliteBlocks::new(
        db.clone(),
        dir.join("files"),
        holder(),
        gate.clone(),
        operation(235),
        8 * 1024 * 1024,
    )?);
    blocks.put(a.clone(), operation(60), original.representation, original.bytes).await?;
    let forged = codec::encode(scope(), b"protected=yes\r\nlabel=old\r\n".to_vec(), Encoding::Raw)?;
    let forged = blocks.put(a.clone(), operation(61), forged.representation, forged.bytes).await?;
    let changed = codec::encode(scope(), b"protected=no\r\nlabel=new\r\n".to_vec(), Encoding::Raw)?;
    let changed = blocks.put(a.clone(), operation(62), changed.representation, changed.bytes).await?;
    let store = engine(db.clone(), blocks.clone(), gate)?;
    let mut valid = request(62, vec![(60, ContentRef::from(changed.content.clone()))]);
    valid.objects[0].base = BaseVersion::Version {
        state_revision: version.clone(),
        content_version: version.clone(),
        root: Some(base.clone()),
    };
    valid.objects[0].before = Some(base.clone());
    valid.objects[0].original = OriginalIntent::Edit {
        edits: vec![TextEdit { search: "label=old\n".into(), replacement: "label=new\n".into(), all: false }],
    };
    let mut wrong = valid.clone();
    wrong.objects[0].before = Some(ContentRef::from(forged.content.clone()));
    assert!(
        matches!(store.submit_pending(principal(),AtomicMutation::new(wrong)?).await,Err(Error::Invalid(reason)) if reason.contains("before root"))
    );
    let mut wrong = valid.clone();
    wrong.objects[0].base = BaseVersion::Version {
        state_revision: version.clone(),
        content_version: version.clone(),
        root: Some(ContentRef::from(forged.content)),
    };
    assert!(
        matches!(store.submit_pending(principal(),AtomicMutation::new(wrong)?).await,Err(Error::Invalid(reason)) if reason.contains("base root"))
    );
    let mut wrong = valid.clone();
    wrong.objects[0].base = BaseVersion::Absent;
    wrong.objects[0].before = None;
    assert!(store.submit_pending(principal(), AtomicMutation::new(wrong)?).await.is_err());
    let mut wrong = valid.clone();
    if let BaseVersion::Version { content_version, .. } = &mut wrong.objects[0].base {
        content_version.revision = Revision(2);
    }
    assert!(store.submit_pending(principal(), AtomicMutation::new(wrong)?).await.is_err());
    assert_eq!(count(&db, "sync_pending_mutations")?, 0);
    store.submit_pending(principal(), AtomicMutation::new(valid.clone())?).await?;
    let mut next = valid;
    next.operation = operation(63);
    next.objects[0].predecessor = Some(operation(62));
    next.objects[0].before = Some(ContentRef::from(changed.content.clone()));
    next.objects[0].original = OriginalIntent::Write { content: ContentRef::from(changed.content), schema: 1 };
    store.submit_pending(principal(), AtomicMutation::new(next.clone())?).await?;
    next.operation = operation(64);
    next.objects[0].before = Some(base);
    assert!(store.submit_pending(principal(), AtomicMutation::new(next)?).await.is_err());
    assert_eq!(count(&db, "sync_pending_mutations")?, 2);
    assert_eq!(count(&db, "sync_object_versions")?, 1);
    Ok(())
}
