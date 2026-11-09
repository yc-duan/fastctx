//! Durable cancellation fences queued publication even when its waiter vanished.
use crate::{
    identity::{OperationId, ScopeKey},
    store::{access::Custody, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) fn intent(db: &Connection, custody: &Custody, operation: &OperationId, cancel: bool) -> Result<()> {
    let scope = &custody.object.scope;
    let holding = serde_json::to_vec(&custody.holding).map_err(|e| sql::invalid(e.to_string()))?;
    let purpose = serde_json::to_vec(&custody.purpose).map_err(|e| sql::invalid(e.to_string()))?;
    let found = db.query_row(
        "SELECT kind,holding,cancelled,purpose FROM sync_asset_uploads WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",
        params![scope.hub.to_string(), scope.scope.as_str(), operation.as_str(), custody.object.object.as_str()],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, bool>(2)?,r.get::<_, Vec<u8>>(3)?)),
    ).optional()?;
    if let Some((kind, previous, cancelled, previous_purpose)) = found {
        if kind != custody.kind.registration().id || previous != holding || previous_purpose != purpose {
            return Err(sql::invalid("upload operation belongs to another kind or holding period"));
        }
        if cancelled && !cancel {
            return Err(sql::invalid("upload operation was cancelled"));
        }
        if cancel && !cancelled {
            db.execute(
                "UPDATE sync_asset_uploads SET cancelled=1 WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",
                params![
                    scope.hub.to_string(),
                    scope.scope.as_str(),
                    operation.as_str(),
                    custody.object.object.as_str()
                ],
            )?;
        }
    } else {
        db.execute("INSERT INTO sync_asset_uploads(hub,scope,operation,object,kind,holding,purpose,cancelled) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![scope.hub.to_string(), scope.scope.as_str(), operation.as_str(), custody.object.object.as_str(), custody.kind.registration().id, holding,purpose,cancel])?;
    }
    Ok(())
}

/// Cancelling an owned operation remains valid after its grant has advanced or
/// disappeared. The persisted holder, object and kind still have to match.
pub(super) fn cancel(
    db: &Connection,
    holder: &crate::cursor::Holder,
    custody: &Custody,
    operation: &OperationId,
) -> Result<()> {
    super::pins::same_holder(holder, custody)?;
    let scope = &custody.object.scope;
    let found = db
        .query_row(
            "SELECT kind,holding FROM sync_asset_uploads WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",
            params![scope.hub.to_string(), scope.scope.as_str(), operation.as_str(), custody.object.object.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;
    if let Some((kind, period)) = found {
        let period: crate::cursor::HoldingPeriod =
            serde_json::from_slice(&period).map_err(|e| sql::invalid(e.to_string()))?;
        if kind != custody.kind.registration().id || &period.holder != holder || period.scope != *scope {
            return Err(sql::invalid("upload cancellation belongs to another object kind or holder"));
        }
        db.execute(
            "UPDATE sync_asset_uploads SET cancelled=1 WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",
            params![scope.hub.to_string(), scope.scope.as_str(), operation.as_str(), custody.object.object.as_str()],
        )?;
        Ok(())
    } else {
        intent(db, custody, operation, true)
    }
}

pub(super) fn batch(
    db: &Connection,
    scope: &ScopeKey,
    only: Option<(&Custody, &OperationId)>,
    max: u32,
) -> Result<u32> {
    let (statement, object, operation, kind) = if let Some((custody, operation)) = only {
        (
            "SELECT operation,object,hash,root_kind FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4 AND kind=?5 AND phase!='released' ORDER BY hash,root_kind LIMIT ?6",
            custody.object.object.as_str(),
            operation.as_str(),
            custody.kind.registration().id,
        )
    } else {
        (
            "SELECT operation,object,hash,root_kind FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND phase!='released' ORDER BY operation,object,kind,hash,root_kind LIMIT ?6",
            "",
            "",
            "",
        )
    };
    let mut query = db.prepare_cached(statement)?;
    let rows = query
        .query_map(params![scope.hub.to_string(), scope.scope.as_str(), operation, object, kind, max], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
        })?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    for (operation, object, hash, form) in &rows {
        db.execute("UPDATE sync_asset_pins SET phase='released' WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4 AND hash=?5 AND root_kind=?6", params![scope.hub.to_string(),scope.scope.as_str(),operation,object,hash,form])?;
        db.execute("INSERT INTO sync_asset_gc(hub,scope,hash,root_kind,generation,next_due) VALUES(?1,?2,?3,?4,?5,0) ON CONFLICT(hub,scope,hash,root_kind) DO NOTHING", params![scope.hub.to_string(),scope.scope.as_str(),hash,form,1_u64.to_be_bytes().as_slice()])?;
    }
    Ok(rows.len() as u32)
}
