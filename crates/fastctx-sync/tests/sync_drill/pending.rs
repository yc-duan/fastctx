//! Atomic local edits survive restarts without borrowing authority revisions.
#[path = "pending_basis.rs"]
mod basis;
#[path = "pending_queue.rs"]
mod queue;
use super::*;
use fastctx_sync::{
    catalog::ObjectHead,
    manifest::ContentRef,
    registry::handlers::{DeletePlan, ExportPlan, KindHandler, MergeBodies, MergeOutcome},
    store::{
        Error,
        authorize::{MutationAuthorizer, PreparedChange},
        engine::{Config, SqliteBackend},
        mutation::{
            AtomicMutation, BaseVersion, FieldEdit, OriginalIntent, ProposedObject, Submission, SubmissionMode,
        },
    },
};

struct TextHandler;
impl KindHandler for TextHandler {
    fn kind(&self) -> &'static str {
        "conversation.plan"
    }
    fn validate(&self, schema: u16, body: &[u8]) -> std::result::Result<(), Error> {
        if schema != 1 {
            return Err(Error::Invalid("unsupported plan schema".into()));
        }
        fastctx_types::plan::validate(std::str::from_utf8(body).map_err(|e| Error::Invalid(e.to_string()))?)
            .map_err(Error::Invalid)
    }
    fn references(&self, _: u16, _: &[u8]) -> std::result::Result<Vec<ContentRef>, Error> {
        Ok(vec![])
    }
    fn import_legacy(&self, _: &str, body: &[u8]) -> std::result::Result<Vec<u8>, Error> {
        self.validate(1, body)?;
        Ok(body.to_vec())
    }
    fn project_legacy(&self, _: &ObjectHead, body: &[u8], _: u16) -> std::result::Result<Vec<u8>, Error> {
        Ok(body.to_vec())
    }
    fn project_local(&self, _: &ObjectHead, body: &[u8]) -> std::result::Result<Vec<u8>, Error> {
        Ok(body.to_vec())
    }
    fn merge(
        &self,
        _: &ProposedObject,
        _: &ObjectHead,
        _: MergeBodies<'_>,
    ) -> std::result::Result<MergeOutcome, Error> {
        Ok(MergeOutcome::Conflict)
    }
    fn apply_edit(
        &self,
        schema: u16,
        current: &[u8],
        edits: &[fastctx_types::sync::mutation::TextEdit],
        max_bytes: usize,
    ) -> std::result::Result<Vec<u8>, Error> {
        self.validate(schema, current)?;
        fastctx_sync::registry::text::apply_edits(current, edits, max_bytes)
    }
    fn patch_fields(&self, _: u16, _: &[u8], _: &[FieldEdit]) -> std::result::Result<Vec<u8>, Error> {
        Err(Error::Invalid("a plan is text".into()))
    }
    fn export(&self, _: &ObjectHead) -> std::result::Result<ExportPlan, Error> {
        Ok(ExportPlan::GenericRootClosure)
    }
    fn delete(&self, _: &ObjectHead) -> std::result::Result<DeletePlan, Error> {
        Ok(DeletePlan::GenericTombstone)
    }
}
struct Permit;
impl MutationAuthorizer for Permit {
    fn kind(&self) -> &'static str {
        "conversation.plan"
    }
    fn authorize(&self, context: &AccessContext, change: PreparedChange<'_>) -> Decision {
        if change.proposal.kind == "conversation.plan" && change.fields.is_empty() {
            Decision::Allowed { revision: context.observed_acl }
        } else {
            Decision::Denied { revision: context.observed_acl, reason: "not the drill's text mutation".into() }
        }
    }
}
fn engine(db: Database, blocks: Arc<SqliteBlocks>, gate: Arc<Policy>) -> Result<SqliteBackend> {
    Ok(SqliteBackend::new(Config {
        database: db,
        hub: scope().hub,
        holder: holder(),
        policy: gate,
        blocks,
        handlers: vec![Arc::new(TextHandler)],
        authorizers: vec![Arc::new(Permit)],
        semantic_bytes: 8 * 1024 * 1024,
    })?)
}
fn principal() -> Principal {
    Principal::Account { account: AccountId::from_bytes([8; 32]), device: DeviceId::from_bytes([3; 32]) }
}
fn request(op: u8, objects: Vec<(u8, ContentRef)>) -> Submission {
    Submission {
        operation: operation(op),
        origin: DeviceId::from_bytes([3; 32]),
        mode: SubmissionMode::Offline,
        objects: objects
            .into_iter()
            .map(|(id, content)| ProposedObject {
                key: custody(id, "conversation.plan").object,
                kind: "conversation.plan".into(),
                observed_acl: Revision(1),
                base: BaseVersion::Absent,
                before: None,
                candidate: Some(content.clone()),
                predecessor: None,
                original: OriginalIntent::Write { content, schema: 1 },
                resolves: vec![],
            })
            .collect(),
    }
}
fn count(db: &Database, table: &str) -> Result<i64> {
    Ok(db.read_now(|db| Ok(db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?))?)
}
pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("pending-edits");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let gate = policy(false);
    let blocks = Arc::new(SqliteBlocks::new(
        db.clone(),
        dir.join("files"),
        holder(),
        gate.clone(),
        operation(230),
        8 * 1024 * 1024,
    )?);
    let store = engine(db.clone(), blocks.clone(), gate.clone())?;
    let content = codec::encode(scope(), b"a retained local plan".to_vec(), Encoding::Raw)?;
    let a = custody(50, "conversation.plan");
    let b = custody(51, "conversation.plan");
    let one = blocks.put(a.clone(), operation(50), content.representation.clone(), content.bytes.clone()).await?;
    let two = blocks.put(b.clone(), operation(50), content.representation, content.bytes).await?;
    let submission =
        request(50, vec![(50, ContentRef::from(one.content.clone())), (51, ContentRef::from(two.content.clone()))]);
    let receipt = store.submit_pending(principal(), AtomicMutation::new(submission.clone())?).await?;
    assert!(receipt.authority.is_none());
    assert_eq!(receipt.local.0, 2);
    assert_eq!(count(&db, "sync_object_versions")?, 0);
    assert_eq!(count(&db, "sync_pending_objects")?, 2);
    assert_eq!(count(&db, "sync_outbox")?, 1);
    blocks.release(a.clone(), one.pin).await?;
    blocks.release(b, two.pin).await?;
    assert_eq!(blocks.collect(128).await?, 0);
    assert_eq!(blocks.read(a.clone(), one.content.id, one.representation).await?, b"a retained local plan");
    assert_eq!(store.submit_pending(principal(), AtomicMutation::new(submission.clone())?).await?.local, receipt.local);
    let mut changed = submission.clone();
    changed.mode = SubmissionMode::Online;
    assert!(store.submit_pending(principal(), AtomicMutation::new(changed)?).await.is_err());
    gate.enabled.store(false, Ordering::Release);
    assert!(store.submit_pending(principal(), AtomicMutation::new(submission.clone())?).await.is_err());
    gate.enabled.store(true, Ordering::Release);
    drop(store);
    drop(blocks);
    drop(db);
    super::super::scheduler::closed(&path).await?;
    let db = open_with(&path, |db| {
        db.execute_batch("CREATE TRIGGER lose_pending_delivery AFTER INSERT ON sync_outbox BEGIN DELETE FROM sync_outbox WHERE destination=NEW.destination AND hub=NEW.hub AND scope=NEW.scope AND operation=NEW.operation; END")?;
        Ok(())
    })?;
    let blocks = Arc::new(SqliteBlocks::new(
        db.clone(),
        dir.join("files"),
        holder(),
        gate.clone(),
        operation(231),
        8 * 1024 * 1024,
    )?);
    let store = engine(db.clone(), blocks.clone(), gate)?;
    assert_eq!(store.submit_pending(principal(), AtomicMutation::new(submission.clone())?).await?.local, receipt.local);
    let mut broken = submission;
    broken.operation = operation(52);
    assert!(store.submit_pending(principal(), AtomicMutation::new(broken)?).await.is_err());
    assert_eq!(count(&db, "sync_pending_mutations")?, 1);
    assert_eq!(count(&db, "sync_pending_objects")?, 2);
    assert_eq!(count(&db, "sync_local_changes")?, 2);
    assert_eq!(count(&db, "sync_outbox")?, 1);
    assert_eq!(count(&db, "sync_pending_roots")?, 2);
    assert_eq!(count(&db, "sync_object_versions")?, 0);
    basis::run(root).await?;
    queue::run(root).await?;
    println!(
        "pending lifecycle: atomic edits, verified bases/predecessors, shared CRLF edits, root retention, replay, restart and rollback passed"
    );
    Ok(())
}
