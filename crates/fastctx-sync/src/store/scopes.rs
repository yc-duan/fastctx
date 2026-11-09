//! Scope rows this World opens for itself. Under its alone hub the World is its
//! own directory, so it opens the scopes of the subjects it hosts there; a scope
//! under any real hub comes only from that hub's directory evidence.
use crate::{
    blocks::SqliteBlocks,
    identity::{DeviceId, HubId, ObjectId, ScopeKey},
    scope::ScopeSubject,
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
pub use advance::ScopeState;
pub(crate) use advance::{advance as advance_in, current};

/// How a scope row came to exist, stored with it. Rows installed from a hub's
/// directory carry that evidence instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
enum Origin {
    /// Opened by this World, identified by its device, for a subject it hosts.
    Hosted { subject: ScopeSubject, device: DeviceId },
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
                    if found != encoded {
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
