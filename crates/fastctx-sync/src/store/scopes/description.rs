//! Upload eligibility reads the description installed by the verified scope port.
use super::Origin;
use crate::{catalog::Lifecycle, identity::ScopeKey, scope::ScopeDescriptor, store::sql};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

/// The lifecycle the installed row records: its hub's description when the hub
/// described it, else what the World hosting it did, which closes a scope only
/// when its subject is deleted. `enabled` is the row's own flag.
pub(crate) fn lifecycle(db: &Connection, scope: &ScopeKey, enabled: bool) -> Result<Lifecycle> {
    let lifecycle = match directory_description(db, scope)? {
        Some(descriptor) => descriptor.lifecycle,
        None if enabled => Lifecycle::Live,
        None => Lifecycle::Deleted,
    };
    if enabled != (lifecycle == Lifecycle::Live) {
        return Err(sql::invalid("the scope row and the description on it disagree"));
    }
    Ok(lifecycle)
}

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
