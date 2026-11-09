//! Public root-use operations delegate to the same SQL primitives as ingestion.
use super::SqliteBlocks;
use crate::{
    blocks::{pins, root_holds},
    identity::{OperationId, ScopeKey},
    store::{
        Error, FutureResult,
        access::Custody,
        boundary::{self, Role},
        roots::{self, AcquireRoot, RootAcquired, RootLease, RootStore},
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
            self.db
                .write(128 * 1024, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Roots);
                    root_holds::retire_scope(db, &scope, max)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
}
impl roots::sealed::RootWriter for SqliteBlocks {}
impl RootStore for SqliteBlocks {
    fn acquire_root(&self, operation: OperationId, request: AcquireRoot) -> FutureResult<'_, RootAcquired> {
        Box::pin(async move {
            request.validate()?;
            let policy = self.policy.clone();
            let holder = self.holder.clone();
            let id = operation.clone();
            self.db
                .write(16384, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Roots);
                    pins::authorize(db, &holder, policy.as_ref(), &request.custody)?;
                    root_holds::acquire(db, &id, &request, now())
                })
                .await
                .map_err(|error| sql::error(error, operation))
        })
    }
    fn release_root(&self, custody: Custody, lease: RootLease) -> FutureResult<'_, ()> {
        Box::pin(async move {
            lease.usage.validate()?;
            lease.root.validate().map_err(Error::Invalid)?;
            if lease.kind != custody.kind.registration().id {
                return Err(Error::Invalid("root usage belongs to another kind".into()));
            }
            let policy = self.policy.clone();
            let holder = self.holder.clone();
            let operation = lease.operation.clone();
            self.db
                .write(16384, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Roots);
                    pins::authorize(db, &holder, policy.as_ref(), &custody)?;
                    root_holds::release(db, &custody, &lease)
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
