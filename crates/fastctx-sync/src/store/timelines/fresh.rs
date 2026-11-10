//! Acquire a current snapshot, retrying only a proven concurrent head replacement.
use super::{SqliteTimelines, state};
use crate::{
    cursor::LocalSequence,
    identity::{Digest, ObjectKey, OperationId, RootLeaseId},
    manifest::ContentRef,
    store::{
        Error,
        access::Custody,
        ingest::StreamProgress,
        roots::{AcquireRoot, AcquireRootError, RootAcquired, RootStore, RootUse},
        sql,
    },
    timeline::TimelineFence,
};
use std::sync::atomic::{AtomicU64, Ordering};

const HOLD_MILLIS: i64 = 10 * 60 * 1000;
const ATTEMPTS: usize = 8;

pub(super) enum Fresh {
    Fence(TimelineFence),
    Pending(Option<StreamProgress>),
}
struct Snapshot {
    head: state::Head,
    progress: StreamProgress,
    custody: Custody,
}
impl SqliteTimelines {
    pub(super) async fn fresh(&self, conversation: &ObjectKey) -> Result<Fresh, Error> {
        let mut snapshot = self.snapshot(conversation).await?;
        for _ in 0..ATTEMPTS {
            let Some(Snapshot { head, progress, custody }) = snapshot else {
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
            match self.blocks.acquire_root(operation, request).await {
                Ok(RootAcquired::Held { .. }) => {
                    return Ok(Fresh::Fence(TimelineFence {
                        conversation: conversation.clone(),
                        projection: head.projection,
                        through: progress,
                        applied: head.applied,
                        local: head.local,
                        root,
                        lease,
                    }));
                }
                Ok(RootAcquired::Pending { .. }) => return Ok(Fresh::Pending(Some(progress))),
                Err(AcquireRootError::TimelineUnbound) => {
                    snapshot = self.snapshot(conversation).await?;
                    if snapshot
                        .as_ref()
                        .is_some_and(|next| next.head.local == head.local && next.head.root == head.root)
                    {
                        return Err(AcquireRootError::TimelineUnbound.into());
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(Error::Busy)
    }

    async fn snapshot(&self, conversation: &ObjectKey) -> Result<Option<Snapshot>, Error> {
        let holder = self.blocks.holder().clone();
        let read = conversation.clone();
        self.blocks
            .database()
            .read(move |db| {
                let Some(head) = state::head(db, &read)? else { return Ok(None) };
                let progress = state::progress(db, &read, &head.through)?;
                Ok(Some(Snapshot { head, progress, custody: state::custody(db, &holder, &read)? }))
            })
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))
    }
}

/// Each reader owns a distinct hold, even when its snapshot is shared.
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
