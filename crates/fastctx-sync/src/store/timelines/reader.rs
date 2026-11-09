//! Pages of a conversation's installed timeline, read locally. A fresh read
//! holds its snapshot root for a while; the current snapshot answers from the
//! position index, an older or rebuilding one from its immutable pages.
use super::{SqliteTimelines, pages::Loaded, state};
use crate::{
    cursor::LocalSequence,
    identity::{Digest, ObjectId, ObjectKey, OperationId, RootLeaseId},
    manifest::ContentRef,
    registry::REGISTRY,
    store::{
        Error, FutureResult, PageLimit,
        access::{AccessContext, Decision},
        ingest::StreamProgress,
        roots::{AcquireRoot, RootAcquired, RootStore, RootUse},
        sql,
    },
    timeline::{
        ItemRead, StreamPage, StreamRead, TimelineFence, TimelineItem, TimelinePage, TimelineRead, TimelineReader,
        TimelineResult,
        pages::{Budget, Outcome},
        query::{self, Head, ItemResult, LeaseState, QueryPlan, Route},
    },
};
use fastctx_storage::rusqlite::params;
use fastctx_types::sync::conversation_timeline::KIND;
use std::sync::atomic::{AtomicU64, Ordering};

/// How long a fresh read holds its snapshot; a reader paging longer re-anchors.
const HOLD_MILLIS: i64 = 10 * 60 * 1000;
/// T2's allowance for decoding one page or item from immutable pages.
const READ_BUDGET: Budget = Budget { work_bytes: 16 * 1024 * 1024, max_pages: 256, max_changes: 0 };

impl TimelineReader for SqliteTimelines {
    fn stream(&self, context: AccessContext, request: StreamRead) -> FutureResult<'_, StreamPage> {
        Box::pin(async move {
            let result = self.read_stream(context.clone(), request).await?;
            self.scope_current(&context)?;
            Ok(result)
        })
    }
    fn timeline(&self, context: AccessContext, request: TimelineRead) -> FutureResult<'_, TimelineResult> {
        Box::pin(async move {
            let conversation = request.conversation.clone();
            let result = self.read_timeline(context.clone(), request).await?;
            self.authorize(&context, &conversation)?;
            if let TimelineResult::Page { page } = &result
                && !self.held(&page.fence).await?
            {
                return Ok(TimelineResult::ReanchorNeeded);
            }
            Ok(result)
        })
    }
    fn item(&self, context: AccessContext, fence: TimelineFence, item: ObjectId) -> FutureResult<'_, ItemRead> {
        Box::pin(async move {
            let result = self.read_item(context.clone(), fence.clone(), item).await?;
            self.authorize(&context, &fence.conversation)?;
            if !self.held(&fence).await? {
                return Ok(ItemRead::ReanchorNeeded);
            }
            Ok(result)
        })
    }
}

enum Fresh {
    Fence(TimelineFence),
    Pending(Option<StreamProgress>),
}

impl SqliteTimelines {
    fn scope_current(&self, context: &AccessContext) -> Result<(), Error> {
        let current = self
            .blocks
            .database()
            .read_now(|db| crate::store::scopes::current(db, &context.scope))
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
        let Some(current) = current.filter(|current| current.enabled) else {
            return Err(Error::Revoked);
        };
        if current.acl_revision != context.observed_acl {
            return Err(Error::Busy);
        }
        Ok(())
    }
    pub(super) fn authorize(&self, context: &AccessContext, conversation: &ObjectKey) -> Result<(), Error> {
        self.authorize_principal(context, conversation)?;
        let deleted = self
            .blocks
            .database()
            .read_now(|db| {
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3)",
                    fastctx_storage::rusqlite::params![
                        conversation.scope.hub.to_string(),
                        conversation.scope.scope.as_str(),
                        conversation.object.as_str()
                    ],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(Into::into)
            })
            .map_err(|e| sql::error(e, self.blocks.worker().clone()))?;
        if deleted { Err(Error::Denied("the conversation was deleted".into())) } else { Ok(()) }
    }
    fn authorize_principal(&self, context: &AccessContext, conversation: &ObjectKey) -> Result<(), Error> {
        self.scope_current(context)?;
        if context.scope != conversation.scope {
            return Err(Error::Denied("the reader's scope differs from the conversation's".into()));
        }
        let kind = REGISTRY.asset(KIND).map_err(Error::Invalid)?.registered();
        match self.blocks.policy().can_read_object(context, kind, conversation) {
            Decision::Allowed { revision } if revision == context.observed_acl => Ok(()),
            Decision::Allowed { .. } => Err(Error::Busy),
            Decision::Denied { reason, .. } => Err(Error::Denied(reason)),
            Decision::Unavailable => Err(Error::Unavailable("the reader's access cannot be decided now".into())),
        }
    }

    /// Ends the snapshot hold behind a fence once its reader is done with it.
    /// Only the exact fence releases its hold; a stale one is already gone.
    pub async fn release(&self, context: AccessContext, fence: TimelineFence) -> Result<(), Error> {
        self.authorize_principal(&context, &fence.conversation)?;
        if !self.held(&fence).await? {
            return Ok(());
        }
        self.blocks.release_use(fence.conversation.scope, fence.lease).await
    }

    async fn held(&self, fence: &TimelineFence) -> Result<bool, Error> {
        let check = fence.clone();
        self.blocks
            .database()
            .read(move |db| state::fence_held(db, &check, now()))
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))
    }

    async fn read_timeline(&self, context: AccessContext, request: TimelineRead) -> Result<TimelineResult, Error> {
        let conversation = request.conversation.clone();
        self.authorize(&context, &conversation)?;
        let limit = PageLimit { items: request.max_items, bytes: request.max_bytes };
        let fence = match request.fence {
            Some(fence) => {
                if fence.conversation != conversation {
                    return Err(Error::Invalid("the fence belongs to another conversation".into()));
                }
                if !self.held(&fence).await? {
                    return Ok(TimelineResult::ReanchorNeeded);
                }
                fence
            }
            None => match self.fresh(&conversation).await? {
                Fresh::Fence(fence) => fence,
                Fresh::Pending(progress) => return Ok(TimelineResult::Pending { progress }),
            },
        };
        let head = self.current(&conversation).await?;
        let plan = query::query_plan(&head, fence, request.anchor, limit)?;
        if plan.route == Route::CurrentSql
            && let Some(page) = self.indexed(&head, &plan).await?
        {
            return Ok(TimelineResult::Page { page: Box::new(page) });
        }
        self.immutable(&plan).await
    }

    async fn read_item(&self, context: AccessContext, fence: TimelineFence, id: ObjectId) -> Result<ItemRead, Error> {
        self.authorize_principal(&context, &fence.conversation)?;
        if !self.held(&fence).await? {
            return Ok(ItemRead::ReanchorNeeded);
        }
        let mut loaded = Loaded::default();
        loop {
            match query::item(&loaded, &fence, &id, LeaseState::Active, READ_BUDGET)? {
                Outcome::Ready(ItemResult::Item(item)) => return Ok(ItemRead::Item { item }),
                Outcome::Ready(ItemResult::ReanchorNeeded) => return Ok(ItemRead::ReanchorNeeded),
                Outcome::Pending { missing } => match loaded.load(&self.blocks, &missing) {
                    Ok(()) => {}
                    Err(Error::Unavailable(_)) => {
                        return Ok(ItemRead::Pending { progress: Some(fence.through.clone()) });
                    }
                    Err(error) => return Err(error),
                },
            }
        }
    }

    /// Holds the current snapshot root and returns the fence that names it.
    async fn fresh(&self, conversation: &ObjectKey) -> Result<Fresh, Error> {
        let holder = self.blocks.holder().clone();
        let read = conversation.clone();
        let snapshot = self
            .blocks
            .database()
            .read(move |db| {
                let Some(head) = state::head(db, &read)? else {
                    return Ok(None);
                };
                let progress = state::progress(db, &read, &head.through)?;
                Ok(Some((head, progress, state::custody(db, &holder, &read)?)))
            })
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
        let Some((head, progress, custody)) = snapshot else {
            return Ok(Fresh::Pending(None));
        };
        let Some(root) = head.root.clone() else {
            return Ok(Fresh::Pending(Some(progress)));
        };
        let lease = lease(conversation, head.local);
        let request = AcquireRoot {
            lease: lease.clone(),
            custody,
            root: ContentRef::Manifest { root: root.clone() },
            usage: RootUse::Timeline { projection: head.projection },
            bases: Vec::new(),
            expires_at_millis: Some(now().saturating_add(HOLD_MILLIS)),
        };
        let operation = OperationId::from_bytes(Digest::of(lease.as_str().as_bytes()).bytes());
        Ok(match self.blocks.acquire_root(operation, request).await? {
            RootAcquired::Held { .. } => Fresh::Fence(TimelineFence {
                conversation: conversation.clone(),
                projection: head.projection,
                through: progress,
                applied: head.applied,
                local: head.local,
                root,
                lease,
            }),
            RootAcquired::Pending { .. } => Fresh::Pending(Some(progress)),
        })
    }

    async fn current(&self, conversation: &ObjectKey) -> Result<Head, Error> {
        let read = conversation.clone();
        let head = self
            .blocks
            .database()
            .read(move |db| state::head(db, &read))
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?
            .ok_or_else(|| Error::Unavailable("the conversation's timeline head disappeared".into()))?;
        let root = head.root.ok_or_else(|| Error::Unavailable("the conversation's timeline has no snapshot".into()))?;
        Ok(Head {
            conversation: conversation.clone(),
            root,
            projection: head.projection,
            indexed_projection: head.indexed,
        })
    }

    /// The current snapshot from the position index, or None when the head
    /// moved since the plan chose this route.
    async fn indexed(&self, head: &Head, plan: &QueryPlan) -> Result<Option<TimelinePage>, Error> {
        let seek = query::sql_seek(plan)?;
        let expected = (head.root.clone(), head.projection, head.indexed_projection);
        let conversation = head.conversation.clone();
        let rows = self
            .blocks
            .database()
            .read(move |db| {
                // A read closure alone is not a SQLite snapshot. Pin the head
                // check and index rows together across concurrent installations.
                let snapshot = db.unchecked_transaction()?;
                let found = state::head(&snapshot, &conversation)?;
                if found.map(|found| (found.root, found.projection, found.indexed))
                    != Some((Some(expected.0), expected.1, expected.2))
                {
                    return Ok(None);
                }
                let scope = &conversation.scope;
                let rows = snapshot
                    .prepare_cached(seek.statement)?
                    .query_map(
                        params![
                            scope.hub.to_string(),
                            scope.scope.as_str(),
                            conversation.object.as_str(),
                            seek.position.clone().unwrap_or_default(),
                            seek.limit
                        ],
                        |row| row.get::<_, Vec<u8>>(0),
                    )?
                    .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
                snapshot.commit()?;
                Ok(Some(rows))
            })
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
        let Some(rows) = rows else {
            return Ok(None);
        };
        let items = rows.into_iter().map(|body| {
            serde_json::from_slice::<TimelineItem>(&body)
                .map_err(|error| Error::Corrupt(format!("an indexed timeline row is not an item: {error}")))
        });
        query::page_from_rows(plan, items).map(Some)
    }

    async fn immutable(&self, plan: &QueryPlan) -> Result<TimelineResult, Error> {
        let mut loaded = Loaded::default();
        loop {
            match query::page(&loaded, plan, LeaseState::Active, READ_BUDGET)? {
                Outcome::Ready(result) => return Ok(result),
                Outcome::Pending { missing } => match loaded.load(&self.blocks, &missing) {
                    Ok(()) => {}
                    Err(Error::Unavailable(_)) => {
                        return Ok(TimelineResult::Pending { progress: Some(plan.fence.through.clone()) });
                    }
                    Err(error) => return Err(error),
                },
            }
        }
    }
}

/// A distinct use per read, so one reader's release never ends another's.
pub(super) fn lease(conversation: &ObjectKey, local: LocalSequence) -> RootLeaseId {
    static READS: AtomicU64 = AtomicU64::new(0);
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    let digest = Digest::of(
        format!(
            "fastctx-sync/timeline-read/v1\0{}\0{}\0{}\0{}\0{}",
            conversation.scope.scope.as_str(),
            conversation.object.as_str(),
            local.0,
            READS.fetch_add(1, Ordering::Relaxed),
            nanos
        )
        .as_bytes(),
    );
    RootLeaseId::from_bytes(digest.bytes())
}

pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}
