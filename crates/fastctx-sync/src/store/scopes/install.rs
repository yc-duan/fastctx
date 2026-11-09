//! A hub's verified scope description installs through this one entry. The row
//! keeps the original proof, a row this World hosted converts to the hub's
//! description, and a changed generation reconciles local custody.
use super::{Origin, ScopeState, SqliteScopes, advance_in, current};
use crate::{
    catalog::Lifecycle,
    cursor::Revision,
    identity::ScopeKey,
    scope::{ScopeDescriptor, ScopeOwner, VerifiedScope, account_scope},
    store::{
        Error, FutureResult,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension, params},
};

/// What installing a description changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Installed {
    /// The scope is described on this device for the first time.
    Opened(ScopeState),
    /// A scope this World hosted now carries its hub's description.
    Converted(ScopeState),
    /// A newer description replaced the installed one.
    Advanced(ScopeState),
    /// The same description was already installed.
    Unchanged(ScopeState),
}

impl SqliteScopes {
    /// Installs a description verified against the pinned hub key. A description
    /// that is not a successor of the installed one is refused.
    pub fn install(&self, verified: &VerifiedScope) -> FutureResult<'_, Installed> {
        let descriptor = verified.descriptor().clone();
        let proof = verified.proof().clone();
        Box::pin(async move {
            let origin = serde_json::to_vec(&Origin::Directory { proof: Box::new(proof) })
                .map_err(|error| Error::Invalid(error.to_string()))?;
            self.blocks
                .database()
                .write(16 * 1024 + origin.len(), move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Scopes);
                    install(db, &descriptor, &origin)
                })
                .await
                .map_err(|error| sql::error(error, self.blocks.worker().clone()))
        })
    }
}

fn install(db: &Connection, descriptor: &ScopeDescriptor, origin: &[u8]) -> SqlResult<Installed> {
    let scope = &descriptor.scope;
    let enabled = descriptor.lifecycle == Lifecycle::Live;
    let directory = match &descriptor.owner {
        ScopeOwner::Account { identity } => Some(account_scope(&identity.id)),
        ScopeOwner::Hub => None,
    };
    let Some(state) = current(db, scope)? else {
        let first = 1_u64.to_be_bytes();
        db.prepare_cached("INSERT INTO sync_scopes(hub,scope,origin,acl_revision,enabled,publication_epoch,directory) VALUES(?1,?2,?3,?4,?5,?6,?7)")?
            .execute(params![
                scope.hub.to_string(),
                scope.scope.as_str(),
                origin,
                descriptor.acl_revision.0.to_be_bytes().as_slice(),
                enabled,
                first.as_slice(),
                directory.as_ref().map(|id| id.as_str())
            ])?;
        return Ok(Installed::Opened(ScopeState {
            acl_revision: descriptor.acl_revision,
            publication_epoch: Revision(1),
            enabled,
        }));
    };
    let converted = match stored(db, scope)? {
        Origin::Hosted { subject, .. } => {
            if !subject.same_subject(&descriptor.subject) {
                return Err(sql::invalid("a hub described another subject under this hosted scope"));
            }
            true
        }
        Origin::Directory { proof } => {
            let previous: ScopeDescriptor =
                serde_json::from_slice(&proof.payload.0).map_err(|error| sql::invalid(error.to_string()))?;
            if previous == *descriptor {
                return Ok(Installed::Unchanged(state));
            }
            descriptor.validate_successor(&previous).map_err(sql::invalid)?;
            false
        }
    };
    db.prepare_cached("UPDATE sync_scopes SET origin=?3,directory=?4 WHERE hub=?1 AND scope=?2")?.execute(params![
        scope.hub.to_string(),
        scope.scope.as_str(),
        origin,
        directory.as_ref().map(|id| id.as_str())
    ])?;
    // Disabling without a new ACL generation still opens a new local tenure,
    // so custody of the enabled period is reconciled away.
    let tenure = if enabled != state.enabled && descriptor.acl_revision == state.acl_revision {
        Revision(state.publication_epoch.0 + 1)
    } else {
        state.publication_epoch
    };
    let next = ScopeState { acl_revision: descriptor.acl_revision, publication_epoch: tenure, enabled };
    if next != state {
        advance_in(db, scope, state, next)?;
    }
    Ok(if converted { Installed::Converted(next) } else { Installed::Advanced(next) })
}

fn stored(db: &Connection, scope: &ScopeKey) -> SqlResult<Origin> {
    let bytes: Vec<u8> = db
        .prepare_cached("SELECT origin FROM sync_scopes WHERE hub=?1 AND scope=?2")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str()], |row| row.get(0))
        .optional()?
        .ok_or_else(|| sql::invalid("the scope row vanished"))?;
    serde_json::from_slice(&bytes).map_err(|error| sql::invalid(error.to_string()))
}
