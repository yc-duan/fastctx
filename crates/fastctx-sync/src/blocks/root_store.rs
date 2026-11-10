//! Public root-use operations delegate to the same SQL primitives as ingestion.
use super::SqliteBlocks;
use crate::{
    blocks::{pins, root_holds},
    identity::{OperationId, RootLeaseId, ScopeKey},
    store::{
        Error, FutureResult,
        access::Custody,
        boundary::{self, Role},
        roots::{self, AcquireRoot, AcquireRootError, AcquireRootResult, RootAcquired, RootLease, RootStore},
        sql,
    },
};

impl SqliteBlocks {
    /// A revocation worker drains usage holds by scope after invalidation and IO
    /// drain. It is not a purge receipt; accepted roots have their own lifecycle.
    pub fn retire_root_uses(&self, scope: ScopeKey, max: u32) -> FutureResult<'_, u32> {
        Box::pin(async move {
            if max == 0 || max > 128 {
                return Err(Error::Invalid("root cleanup needs a page of 1 to 128 uses".into()));
            }
            if !self.close(&scope)? {
                return Err(Error::Busy);
            }
            let holder = self.holder.clone();
            let policy = self.policy.clone();
            self.db
                .write(128 * 1024, move |db| {
                    let writer = crate::blocks::supply::writer(db, &holder, policy.as_ref(), now())?;
                    let invalidated = writer.invalidate(&scope, None, None, None, max.min(64))?;
                    let _boundary = boundary::enter_typed_write(Role::Roots);
                    Ok(root_holds::retire_scope(db, &scope, max)? + invalidated.changed)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
}
impl SqliteBlocks {
    /// Ends a use known by its identity alone, such as the hold behind a
    /// reader's fence; releasing it twice is harmless.
    pub(crate) fn release_use(&self, scope: ScopeKey, lease: RootLeaseId) -> FutureResult<'_, ()> {
        Box::pin(async move {
            let holder = self.holder.clone();
            self.db
                .write(16384, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Roots);
                    root_holds::release_id(db, &holder, &scope, &lease)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
}
impl roots::sealed::RootWriter for SqliteBlocks {}
impl RootStore for SqliteBlocks {
    fn acquire_root(&self, operation: OperationId, request: AcquireRoot) -> AcquireRootResult<'_> {
        Box::pin(async move {
            request.validate()?;
            let policy = self.policy.clone();
            let holder = self.holder.clone();
            let id = operation.clone();
            self.db
                .write(2 * 1024 * 1024, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Roots);
                    pins::authorize(db, &holder, policy.as_ref(), &request.custody)?;
                    let now = now();
                    if let Some(previous) = root_holds::existing(db, &id, &request, now)? {
                        return Ok(Some(previous));
                    }
                    let writer = crate::blocks::supply::writer(db, &holder, policy.as_ref(), now)?;
                    if matches!(request.usage, crate::store::roots::RootUse::Timeline { .. }) {
                        return writer.acquire_timeline(&id, &request);
                    }
                    let supply = crate::blocks::supply::SupplyRequest {
                        operation: id.clone(),
                        custody: request.custody.clone(),
                        root: request.root.clone(),
                        bases: request.bases.clone(),
                        owner: crate::blocks::supply::SupplyOwner::Use { usage: request.usage.clone() },
                    };
                    let ticket = writer.begin(&supply)?;
                    match writer.step(&ticket, crate::blocks::supply::SupplyBudget::default())? {
                        crate::blocks::supply::SupplyProgress::Ready => {
                            let ready = writer.recheck(&request.custody, &ticket)?;
                            Ok(Some(RootAcquired::Held { lease: Box::new(writer.acquire(ready, &request)?) }))
                        }
                        crate::blocks::supply::SupplyProgress::Pending { .. } => {
                            Ok(Some(RootAcquired::Pending { completeness: crate::cursor::Completeness::MetadataOnly }))
                        }
                        crate::blocks::supply::SupplyProgress::Rejected { node, reason } => Err(sql::invalid(format!(
                            "content {} has no qualified supply: {reason:?}",
                            node.id().digest.as_str()
                        ))),
                        _ => Err(fastctx_storage::Error::with_kind(
                            fastctx_storage::ErrorKind::Busy,
                            "root supply changed before acquisition",
                        )),
                    }
                })
                .await
                .map_err(|error| AcquireRootError::Store(sql::error(error, operation)))?
                .ok_or(AcquireRootError::TimelineUnbound)
        })
    }
    fn release_root(&self, custody: Custody, lease: RootLease) -> FutureResult<'_, ()> {
        Box::pin(async move {
            lease.usage.validate()?;
            lease.root.validate().map_err(Error::Invalid)?;
            if lease.kind != custody.kind.registration().id {
                return Err(Error::Invalid("root usage belongs to another kind".into()));
            }
            let holder = self.holder.clone();
            let operation = lease.operation.clone();
            self.db
                .write(16384, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Roots);
                    root_holds::release(db, &holder, &custody, &lease)
                })
                .await
                .map_err(|error| sql::error(error, operation))
        })
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}
