//! Scope rows on a device: the scopes of conversations this World hosts, which
//! it opens itself, and the scopes its hubs describe, installed from their
//! signed descriptions. A hosted scope under a real hub converts in place once
//! that hub describes it.
use crate::{
    blocks::SqliteBlocks,
    identity::{DeviceId, HubId, ObjectId, ScopeKey},
    scope::{ScopeDescriptor, ScopeProof, ScopeSubject},
    store::{
        Error, FutureResult,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::rusqlite::params;
use fastctx_types::sync::conversation_identity::conversation_scope;
use serde::{Deserialize, Serialize};

mod advance;
mod install;
pub use advance::ScopeState;
pub(crate) use advance::{advance as advance_in, current};
pub use install::Installed;

/// How a scope row came to exist, stored with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
enum Origin {
    /// Opened by this World, identified by its device, for a subject it hosts.
    Hosted { subject: ScopeSubject, device: DeviceId },
    /// Installed from the hub's latest signed description, with its proof.
    Directory { proof: Box<ScopeProof> },
}
impl Origin {
    /// Whether this row stands for the scope a hosted opening asks for: the same
    /// opening, or the hub's description of the same subject.
    fn hosts(&self, opening: &Self) -> Result<bool, Error> {
        let Self::Hosted { subject: wanted, .. } = opening else { return Ok(false) };
        Ok(match self {
            Self::Hosted { .. } => self == opening,
            Self::Directory { proof } => {
                let descriptor: ScopeDescriptor =
                    serde_json::from_slice(&proof.payload.0).map_err(|error| Error::Invalid(error.to_string()))?;
                descriptor.subject.same_subject(wanted)
            }
        })
    }
}

#[derive(Clone)]
pub struct SqliteScopes {
    blocks: SqliteBlocks,
    alone: HubId,
    device: DeviceId,
}
impl SqliteScopes {
    /// `alone` is the World's own hub, `device` this World's device identity.
    pub fn new(blocks: SqliteBlocks, alone: HubId, device: DeviceId) -> Self {
        Self { blocks, alone, device }
    }

    pub(crate) fn device(&self) -> &DeviceId {
        &self.device
    }

    /// Opens, or confirms, the scope of a conversation this World hosts under its
    /// alone hub. `conversation` is the canonical conversation identity, and the
    /// scope is derived from it rather than chosen. A scope closed because its
    /// conversation was deleted is not reopened.
    pub fn open_conversation(&self, conversation: ObjectId) -> FutureResult<'_, ScopeKey> {
        Box::pin(async move {
            let scope = ScopeKey { hub: self.alone.clone(), scope: conversation_scope(&conversation) };
            let origin = Origin::Hosted {
                subject: ScopeSubject::Conversation { conversation, shared_on: None, born_seal: None },
                device: self.device.clone(),
            };
            let encoded = serde_json::to_vec(&origin).map_err(|error| Error::Invalid(error.to_string()))?;
            let key = scope.clone();
            let open = self
                .blocks
                .database()
                .write(4096 + encoded.len(), move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Scopes);
                    let hub = key.hub.to_string();
                    let first = 1_u64.to_be_bytes();
                    db.prepare_cached("INSERT INTO sync_scopes(hub,scope,origin,acl_revision,enabled,publication_epoch) VALUES(?1,?2,?3,?4,1,?4) ON CONFLICT(hub,scope) DO NOTHING")?
                        .execute(params![hub, key.scope.as_str(), encoded, first.as_slice()])?;
                    let (found, enabled) = db
                        .prepare_cached("SELECT origin,enabled FROM sync_scopes WHERE hub=?1 AND scope=?2")?
                        .query_row(params![hub, key.scope.as_str()], |row| {
                            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, bool>(1)?))
                        })?;
                    let found: Origin = serde_json::from_slice(&found).map_err(|error| sql::invalid(error.to_string()))?;
                    if !found.hosts(&origin).map_err(|error| sql::invalid(error.to_string()))? {
                        return Err(sql::invalid("the scope was opened with another origin or subject"));
                    }
                    Ok(enabled)
                })
                .await
                .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
            if !open {
                return Err(Error::Revoked);
            }
            Ok(scope)
        })
    }
}
