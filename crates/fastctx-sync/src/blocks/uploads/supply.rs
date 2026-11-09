//! A single durable fence covers every pin used by an operation on one object.
use super::*;
use crate::{
    cursor::Revision,
    identity::{ScopeId, SourceId},
};

pub(in crate::blocks) struct ActiveSupply {
    pub revision: Revision,
    pub source: Option<StepKey>,
}

pub(in crate::blocks) fn active_supply(
    db: &Connection,
    custody: &Custody,
    operation: &OperationId,
) -> Result<Option<ActiveSupply>> {
    let Some(found) = saved(db, custody, operation)? else {
        return Ok(None);
    };
    let holding = serde_json::to_vec(&custody.holding).map_err(|e| sql::invalid(e.to_string()))?;
    let purpose = serde_json::to_vec(&custody.purpose).map_err(|e| sql::invalid(e.to_string()))?;
    if found.kind != custody.kind.registration().id || found.holding != holding || found.purpose != purpose {
        return Err(sql::invalid("supply intent differs from the requested object custody"));
    }
    if found.cancelled || found.consumed {
        return Ok(None);
    }
    let scope = &custody.object.scope;
    let source = found
        .step
        .map(|(scope_name, source, revision)| {
            let revision: [u8; 8] =
                revision.try_into().map_err(|_| sql::invalid("source step has an invalid revision"))?;
            Ok::<_, fastctx_storage::Error>(StepKey {
                scope: ScopeKey { hub: scope.hub.clone(), scope: ScopeId::try_from(scope_name).map_err(sql::invalid)? },
                source: SourceId::try_from(source).map_err(sql::invalid)?,
                revision: Revision(u64::from_be_bytes(revision)),
            })
        })
        .transpose()?;
    if let Some(source) = &source
        && !source_step_active(db, &scope.hub, source, operation, custody)?
    {
        return Ok(None);
    }
    let revision = db.query_row(
        "SELECT supply_revision FROM sync_asset_uploads WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",
        params![scope.hub.to_string(), scope.scope.as_str(), operation.as_str(), custody.object.object.as_str()],
        |row| sql::counter(row, 0),
    )?;
    Ok(Some(ActiveSupply { revision: Revision(revision), source }))
}

pub(in crate::blocks) fn advance_supply(
    db: &Connection,
    hub: &str,
    scope: &str,
    operation: &str,
    object: &str,
) -> Result<()> {
    let previous = db.query_row(
        "SELECT supply_revision FROM sync_asset_uploads WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",
        params![hub, scope, operation, object],
        |row| sql::counter(row, 0),
    )?;
    let next = previous.checked_add(1).ok_or_else(|| sql::invalid("supply revision exhausted"))?;
    db.execute(
        "UPDATE sync_asset_uploads SET supply_revision=?1 WHERE hub=?2 AND scope=?3 AND operation=?4 AND object=?5",
        params![next.to_be_bytes().as_slice(), hub, scope, operation, object],
    )?;
    Ok(())
}
