//! Only explicit usage expiry and disabled custody retire these local holds.
use super::*;
use crate::identity::{HubId, ScopeId};

pub(in crate::blocks) fn expire(db: &Connection, now: i64, max: u32) -> Result<()> {
    let mut query = db.prepare_cached(
        "SELECT hub,scope,lease,hash,root_kind FROM sync_root_leases WHERE released=0 AND expires<=?1 ORDER BY expires,hub,scope,lease LIMIT ?2",
    )?;
    let expired =
        query.query_map(params![now, max], candidate)?.collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    for (scope, id, hash, form) in expired {
        retire(db, &scope, &id, &hash, &form)?;
    }
    Ok(())
}

pub(in crate::blocks) fn retire_scope(db: &Connection, scope: &ScopeKey, max: u32) -> Result<u32> {
    let enabled: bool = db.query_row(
        "SELECT enabled FROM sync_scopes WHERE hub=?1 AND scope=?2",
        params![scope.hub.to_string(), scope.scope.as_str()],
        |row| row.get(0),
    )?;
    if enabled {
        return Err(sql::invalid("root-use cleanup requires a durably invalidated scope"));
    }
    let mut query = db.prepare_cached(
        "SELECT hub,scope,lease,hash,root_kind FROM sync_root_leases INDEXED BY sync_root_leases_scope WHERE hub=?1 AND scope=?2 AND released=0 ORDER BY lease LIMIT ?3",
    )?;
    let disabled = query
        .query_map(params![scope.hub.to_string(), scope.scope.as_str(), max], candidate)?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    let count = disabled.len() as u32;
    for (scope, id, hash, form) in disabled {
        retire(db, &scope, &id, &hash, &form)?;
    }
    Ok(count)
}

fn candidate(
    row: &fastctx_storage::rusqlite::Row<'_>,
) -> fastctx_storage::rusqlite::Result<(ScopeKey, RootLeaseId, String, String)> {
    let invalid = |index, error: String| {
        fastctx_storage::rusqlite::Error::FromSqlConversionFailure(
            index,
            fastctx_storage::rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
        )
    };
    Ok((
        ScopeKey {
            hub: HubId::try_from(row.get::<_, String>(0)?).map_err(|e| invalid(0, e))?,
            scope: ScopeId::try_from(row.get::<_, String>(1)?).map_err(|e| invalid(1, e))?,
        },
        RootLeaseId::try_from(row.get::<_, String>(2)?).map_err(|e| invalid(2, e))?,
        row.get(3)?,
        row.get(4)?,
    ))
}
