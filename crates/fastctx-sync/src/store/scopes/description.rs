//! Upload eligibility reads the description installed by the verified scope port.
use super::Origin;
use crate::{identity::ScopeKey, scope::ScopeDescriptor, store::sql};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(crate) fn directory_description(db: &Connection, scope: &ScopeKey) -> Result<Option<ScopeDescriptor>> {
    let bytes = db
        .query_row(
            "SELECT origin FROM sync_scopes WHERE hub=?1 AND scope=?2",
            params![scope.hub.to_string(), scope.scope.as_str()],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    let Some(bytes) = bytes else { return Ok(None) };
    let origin: Origin = serde_json::from_slice(&bytes).map_err(|e| sql::invalid(e.to_string()))?;
    match origin {
        Origin::Hosted { .. } => Ok(None),
        Origin::Directory { proof } => {
            let descriptor: ScopeDescriptor =
                serde_json::from_slice(&proof.payload.0).map_err(|e| sql::invalid(e.to_string()))?;
            if descriptor.scope != *scope {
                return Err(sql::invalid("installed description differs from its scope"));
            }
            Ok(Some(descriptor))
        }
    }
}
