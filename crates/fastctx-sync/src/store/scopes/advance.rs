//! A committed scope fence and its resumable maintenance intent advance together.
use super::*;
use crate::cursor::{LocalSequence, Revision};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension},
};

/// Local custody coordinates installed by the trusted directory adapter. This
/// does not replace its evidence checks or grant any principal a new privilege.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeState {
    pub acl_revision: Revision,
    pub publication_epoch: Revision,
    pub enabled: bool,
}
impl SqliteScopes {
    pub fn current(&self, scope: ScopeKey) -> FutureResult<'_, Option<ScopeState>> {
        Box::pin(async move {
            self.blocks
                .database()
                .read(move |db| current(db, &scope))
                .await
                .map_err(|error| sql::error(error, self.blocks.worker().clone()))
        })
    }
    /// The caller has authenticated the directory change before entering this
    /// port. An identical retry succeeds; a stale expectation or rollback does
    /// not overwrite a newer fence. New scopes still need their origin installed.
    pub fn advance(&self, scope: ScopeKey, expected: ScopeState, next: ScopeState) -> FutureResult<'_, ScopeState> {
        Box::pin(async move {
            let advanced = self
                .blocks
                .database()
                .write(16 * 1024, move |db| super::advance_in(db, &scope, expected, next))
                .await
                .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
            self.blocks.moved().notify_waiters();
            Ok(advanced)
        })
    }
}
pub(crate) fn current(db: &Connection, scope: &ScopeKey) -> SqlResult<Option<ScopeState>> {
    db.query_row(
        "SELECT acl_revision,publication_epoch,enabled FROM sync_scopes WHERE hub=?1 AND scope=?2",
        params![scope.hub.to_string(), scope.scope.as_str()],
        |row| {
            Ok(ScopeState {
                acl_revision: Revision(sql::counter(row, 0)?),
                publication_epoch: Revision(sql::counter(row, 1)?),
                enabled: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}
/// Shares the directory installer's existing transaction; no IO or commit occurs here.
pub(crate) fn advance(
    db: &Connection,
    scope: &ScopeKey,
    expected: ScopeState,
    next: ScopeState,
) -> SqlResult<ScopeState> {
    if db.is_autocommit() {
        return Err(sql::invalid("scope advancement needs its owning write transaction"));
    }
    let _boundary = boundary::enter_typed_write(Role::Scopes);
    let found =
        current(db, scope)?.ok_or_else(|| sql::invalid("a scope must be installed before its fence advances"))?;
    if found == next {
        return Ok(found);
    }
    if found != expected {
        return Err(fastctx_storage::Error::with_kind(
            fastctx_storage::ErrorKind::Busy,
            "the scope fence advanced before this update",
        ));
    }
    if next.acl_revision < found.acl_revision
        || next.publication_epoch < found.publication_epoch
        || (next.acl_revision == found.acl_revision && next.publication_epoch == found.publication_epoch)
    {
        return Err(sql::invalid("scope generations must advance without rolling either coordinate back"));
    }
    let hub = scope.hub.to_string();
    let generation = db
        .query_row(
            "SELECT generation FROM sync_scope_reconciliations WHERE hub=?1 AND scope=?2",
            params![hub, scope.scope.as_str()],
            |row| sql::counter(row, 0),
        )
        .optional()?
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| sql::invalid("scope reconciliation generation exhausted"))?;
    db.execute(
        "UPDATE sync_scopes SET acl_revision=?3,publication_epoch=?4,enabled=?5 WHERE hub=?1 AND scope=?2",
        params![
            hub,
            scope.scope.as_str(),
            next.acl_revision.0.to_be_bytes().as_slice(),
            next.publication_epoch.0.to_be_bytes().as_slice(),
            next.enabled
        ],
    )?;
    crate::blocks::reconcile::begin(db, scope, Revision(generation), next)?;
    crate::scheduler::scope_changed(db, scope, LocalSequence(generation))?;
    Ok(next)
}
