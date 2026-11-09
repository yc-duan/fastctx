//! Cancellation and revocation retire temporary pins without expiring assets.
use super::SqliteBlocks;
use crate::{
    blocks::{pins, uploads},
    identity::{OperationId, ScopeKey},
    store::{
        Error, FutureResult,
        access::Custody,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::rusqlite::params;

impl SqliteBlocks {
    /// Records cancellation even if no pin has arrived yet. Repeated bounded
    /// calls release the operation's pins; zero means its temporary holds drained.
    /// Accepted and pending-edit roots remain independently protected.
    pub fn cancel_upload(&self, custody: Custody, operation: OperationId, max: u32) -> FutureResult<'_, u32> {
        Box::pin(async move {
            page_size(max)?;
            let policy = self.policy.clone();
            let holder = self.holder.clone();
            let id = operation.clone();
            self.db
                .write(128 * 1024, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Content);
                    pins::authorize(db, &holder, policy.as_ref(), &custody)?;
                    uploads::intent(db, &custody, &id, true)?;
                    uploads::batch(db, &custody.object.scope, Some((&custody, &id)), max)
                })
                .await
                .map_err(|e| sql::error(e, operation))
        })
    }
    /// Releases one page of pins after durable invalidation and IO drain. This
    /// is not a purge receipt: roots, transfers and working copies have separate
    /// cleanup stages. A source awaiting the hub's ACK is not invalidated yet.
    pub fn retire_uploads(&self, scope: ScopeKey, max: u32) -> FutureResult<'_, u32> {
        Box::pin(async move {
            page_size(max)?;
            if !self.close(&scope)? {
                return Err(Error::Busy);
            }
            self.db
                .write(128 * 1024, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Content);
                    let enabled: bool = db.query_row(
                        "SELECT enabled FROM sync_scopes WHERE hub=?1 AND scope=?2",
                        params![scope.hub.to_string(), scope.scope.as_str()],
                        |row| row.get(0),
                    )?;
                    if enabled {
                        return Err(sql::invalid("content cleanup requires a durably invalidated scope"));
                    }
                    uploads::batch(db, &scope, None, max)
                })
                .await
                .map_err(|e| sql::error(e, self.worker.clone()))
        })
    }
}
fn page_size(max: u32) -> Result<(), Error> {
    if max == 0 || max > 128 {
        return Err(Error::Invalid("upload cleanup needs a page of 1 to 128 pins".into()));
    }
    Ok(())
}
