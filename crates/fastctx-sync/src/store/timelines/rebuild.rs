//! Resume a position-index rebuild in bounded pages. The immutable head remains
//! authoritative until copying and retirement finish under its final CAS.
use super::{SqliteTimelines, pages::Loaded, reader, state::Head};
use crate::{
    cursor::{Revision, Sequence},
    identity::{ObjectId, ObjectKey, OperationId},
    manifest::{ContentRef, ManifestRoot},
    scheduler::{Lease, Outcome},
    store::{
        Error,
        boundary::{self, Role},
        roots::{AcquireRoot, RootAcquired, RootStore, RootUse},
        sql,
    },
    timeline::{
        pages::{Budget, Outcome as Planned},
        position,
        scan::{self, IndexPage},
        update::Projection,
    },
};
use fastctx_types::sync::transport_codec::bounded_json;
mod state;

const ROWS: u32 = 64;
const ROW_BYTES: usize = 1024 * 1024;
const PAGE_BUDGET: Budget = Budget { work_bytes: 64 * 1024 * 1024, max_pages: 128, max_changes: 0 };
struct Row {
    id: ObjectId,
    revision: Revision,
    accepted: Sequence,
    position: Vec<u8>,
    body: Vec<u8>,
    turn: Option<ObjectId>,
    parent: Option<ObjectId>,
}
enum Prepared {
    Copy { rows: Vec<Row>, done: bool },
    Retire,
}
enum Step {
    More,
    Moved,
    Complete,
}

pub(super) async fn process(timelines: &SqliteTimelines, lease: Lease) -> Result<Outcome, Error> {
    let object = conversation(&lease)?;
    let blocks = &timelines.blocks;
    let holder = blocks.holder().clone();
    let read = object.clone();
    let snapshot = blocks
        .database()
        .read(move |db| {
            let Some(head) = super::state::head(db, &read)? else { return Ok(None) };
            if head.indexed == Some(head.projection) {
                return Ok(None);
            }
            Ok(Some((head, state::progress(db, &read)?, super::state::custody(db, &holder, &read)?)))
        })
        .await
        .map_err(|e| sql::error(e, blocks.worker().clone()))?;
    let Some((head, progress, custody)) = snapshot else {
        return Ok(Outcome::Completed { through: lease.task.wanted });
    };
    let root = head.root.clone().ok_or_else(|| Error::Unavailable("a pending index has no snapshot root".into()))?;
    let id = reader::lease(&object, head.local);
    let operation = OperationId::from_bytes(crate::identity::Digest::of(id.as_str().as_bytes()).bytes());
    let acquired = blocks
        .acquire_root(
            operation.clone(),
            AcquireRoot {
                lease: id.clone(),
                custody,
                root: ContentRef::Manifest { root },
                usage: RootUse::Timeline { projection: head.projection },
                bases: Vec::new(),
                expires_at_millis: Some(reader::now().saturating_add(60_000)),
            },
        )
        .await;
    let result = match acquired {
        Ok(RootAcquired::Held { .. }) => match prepare(timelines, &object, &head, progress.as_ref()) {
            Ok(prepared) => {
                let weight = match &prepared {
                    Prepared::Copy { rows, .. } => {
                        rows.iter().map(|row| row.body.len() + row.position.len() + 1024).sum::<usize>()
                            + fastctx_types::sync::PAGE_BYTES as usize
                    }
                    Prepared::Retire => 64 * 1024,
                };
                let write = object.clone();
                let target = head.clone();
                blocks
                    .database()
                    .write(weight, move |db| {
                        let _boundary = boundary::enter_typed_write(Role::Projection);
                        state::commit(db, &write, &target, &progress, &prepared)
                    })
                    .await
                    .map_err(|e| sql::error(e, operation.clone()))
            }
            Err(error) => Err(error),
        },
        Ok(RootAcquired::Pending { .. }) => Err(Error::Unavailable("the index snapshot root is not local yet".into())),
        Err(error) => Err(error),
    };
    // A cancelled process leaves only a finite hold; restart resumes from the
    // persisted row cursor, never from this process's transient root lease.
    blocks.release_use(object.scope.clone(), id).await?;
    let step = match result {
        Ok(step) => step,
        Err(error) => {
            let read = object.clone();
            let moved = blocks
                .database()
                .read(move |db| Ok(super::state::head(db, &read)?.is_none_or(|found| !state::matches(&found, &head))))
                .await
                .map_err(|e| sql::error(e, operation))?;
            if !moved {
                return Err(error);
            }
            Step::Moved
        }
    };
    Ok(match step {
        Step::Complete => Outcome::Completed { through: lease.task.wanted },
        Step::More => Outcome::Retry { next_due_millis: 0, reason: "more position index rows to rebuild".into() },
        Step::Moved => Outcome::Retry { next_due_millis: 0, reason: "the index rebuild head or progress moved".into() },
    })
}
fn prepare(
    timelines: &SqliteTimelines,
    object: &ObjectKey,
    head: &Head,
    progress: Option<&state::Progress>,
) -> Result<Prepared, Error> {
    let active = progress.filter(|progress| state::current(progress, head));
    if active.is_some_and(|progress| progress.retiring) {
        return Ok(Prepared::Retire);
    }
    let after = active.and_then(|progress| progress.after.as_deref()).map(position::decode).transpose()?;
    let expected =
        Projection { conversation: object.clone(), revision: head.projection, through: head.through.clone() };
    let root = head.root.as_ref().expect("snapshot checked");
    let mut loaded = Loaded::default();
    let mut take = ROWS;
    let page = loop {
        match scan::index_page(&loaded, root, &expected, after.as_ref(), take, PAGE_BUDGET) {
            Ok(Planned::Ready(page)) => break page,
            Ok(Planned::Pending { missing }) => loaded.load(&timelines.blocks, &missing)?,
            Err(Error::Busy) if take > 1 => take /= 2,
            Err(error) => return Err(error),
        }
    };
    rows(page)
}
fn rows(page: IndexPage) -> Result<Prepared, Error> {
    let mut rows = Vec::new();
    let mut bytes = 0;
    let mut done = page.done;
    for item in page.items {
        let body = bounded_json(&item, fastctx_types::sync::PAGE_BYTES as usize).map_err(|_| Error::Busy)?;
        let position = position::encode(&item.position)?;
        let count = body.len() + position.len() + 1024;
        if bytes + count > ROW_BYTES && !rows.is_empty() {
            done = false;
            break;
        }
        bytes += count;
        rows.push(Row {
            id: item.id,
            revision: item.revision,
            accepted: item.position.accepted,
            position,
            body,
            turn: item.turn,
            parent: item.parent,
        });
    }
    Ok(Prepared::Copy { rows, done })
}
fn conversation(lease: &Lease) -> Result<ObjectKey, Error> {
    if lease.task.key.kind != "sync.projection" {
        return Err(Error::Invalid("index rebuild received another handler's lease".into()));
    }
    let id = lease
        .task
        .key
        .key
        .strip_prefix("derived.timeline-index/")
        .ok_or_else(|| Error::Invalid("not a timeline index rebuild task".into()))?;
    Ok(ObjectKey {
        scope: lease.task.key.scope.clone(),
        object: ObjectId::try_from(id.to_owned()).map_err(Error::Invalid)?,
    })
}
