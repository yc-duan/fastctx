//! The hub's scope rows: what this hub last signed for each scope it describes.
use crate::{
    cursor::Revision,
    identity::{FeedEpoch, ScopeKey},
    scope::{AccountIdentity, ScopeDescriptor, ScopeProof},
    store::{
        control::AuthorityRun,
        feed,
        scopes::{self, ScopeState},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use serde::{Deserialize, Serialize};

/// The `origin` of a row this hub describes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Described {
    /// The account's directory is open while its own description is written.
    Opening { owner: AccountIdentity },
    /// The latest description this hub signed, with its original proof.
    Directory { proof: Box<ScopeProof> },
}
impl Described {
    pub(super) fn descriptor(&self) -> Result<Option<ScopeDescriptor>> {
        match self {
            Self::Opening { .. } => Ok(None),
            Self::Directory { proof } => {
                serde_json::from_slice(&proof.payload.0).map(Some).map_err(|error| sql::invalid(error.to_string()))
            }
        }
    }
}

pub(super) struct Row {
    pub origin: Described,
    pub acl: Revision,
    pub publication: Revision,
}

pub(super) fn get(db: &Connection, scope: &ScopeKey) -> Result<Option<Row>> {
    db.prepare_cached("SELECT origin,acl_revision,publication_epoch FROM sync_scopes WHERE hub=?1 AND scope=?2")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, sql::counter(row, 1)?, sql::counter(row, 2)?))
        })
        .optional()?
        .map(|(origin, acl, publication)| {
            Ok(Row {
                origin: serde_json::from_slice(&origin).map_err(|error| sql::invalid(error.to_string()))?,
                acl: Revision(acl),
                publication: Revision(publication),
            })
        })
        .transpose()
}

/// Opens an account's own scope row before its first description exists.
pub(super) fn open_directory(
    db: &Connection,
    run: &AuthorityRun,
    directory: &ScopeKey,
    owner: &AccountIdentity,
) -> Result<()> {
    let origin = encode(&Described::Opening { owner: owner.clone() })?;
    insert(db, directory, &origin, Revision(1), true, directory)?;
    feed::open(db, directory, &FeedEpoch::from_bytes(run.draw()?)).map(drop)
}

/// Records a signed description on its scope's row, opening the row and its
/// feed for a scope described for the first time. A changed ACL generation
/// advances the row's fence through the same reconciliation every holder uses.
pub(super) fn describe(
    db: &Connection,
    run: &AuthorityRun,
    directory: &ScopeKey,
    descriptor: &ScopeDescriptor,
    proof: &ScopeProof,
) -> Result<()> {
    let scope = &descriptor.scope;
    let origin = encode(&Described::Directory { proof: Box::new(proof.clone()) })?;
    let enabled = descriptor.lifecycle == crate::catalog::Lifecycle::Live;
    let Some(found) = scopes::current(db, scope)? else {
        insert(db, scope, &origin, descriptor.acl_revision, enabled, directory)?;
        return feed::open(db, scope, &FeedEpoch::from_bytes(run.draw()?)).map(drop);
    };
    db.prepare_cached("UPDATE sync_scopes SET origin=?3,directory=?4 WHERE hub=?1 AND scope=?2")?.execute(params![
        scope.hub.to_string(),
        scope.scope.as_str(),
        origin,
        directory.scope.as_str()
    ])?;
    let next =
        ScopeState { acl_revision: descriptor.acl_revision, publication_epoch: found.publication_epoch, enabled };
    if next != found {
        scopes::advance_in(db, scope, found, next)?;
    }
    Ok(())
}

fn insert(
    db: &Connection,
    scope: &ScopeKey,
    origin: &[u8],
    acl: Revision,
    enabled: bool,
    directory: &ScopeKey,
) -> Result<()> {
    let first = 1_u64.to_be_bytes();
    db.prepare_cached("INSERT INTO sync_scopes(hub,scope,origin,acl_revision,enabled,publication_epoch,directory) VALUES(?1,?2,?3,?4,?5,?6,?7)")?
        .execute(params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            origin,
            acl.0.to_be_bytes().as_slice(),
            enabled,
            first.as_slice(),
            directory.scope.as_str()
        ])?;
    Ok(())
}

fn encode(origin: &Described) -> Result<Vec<u8>> {
    serde_json::to_vec(origin).map_err(|error| sql::invalid(error.to_string()))
}
