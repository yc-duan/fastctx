//! Durable cancellation fences queued publication even when its waiter vanished.
//! Source uploads also name their engine-issued step, which alone may consume or
//! cancel them; a source operation cannot be reused by an ordinary upload.
use crate::{
    identity::{HubId, OperationId, ScopeKey},
    store::{access::Custody, source_step::StepKey, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

/// Who owns an operation's uploads: an ordinary replica edit or a source step.
#[derive(Clone, Copy)]
pub(crate) enum Owner<'a> {
    Replica,
    Source(&'a StepKey),
}

mod supply;
pub(super) use supply::{ActiveSupply, active_supply, advance_supply};

struct Saved {
    kind: String,
    holding: Vec<u8>,
    purpose: Vec<u8>,
    cancelled: bool,
    consumed: bool,
    step: Option<(String, String, Vec<u8>)>,
}
fn saved(db: &Connection, custody: &Custody, operation: &OperationId) -> Result<Option<Saved>> {
    let scope = &custody.object.scope;
    db.query_row(
        "SELECT kind,holding,purpose,cancelled,consumed,source_scope,source,source_step FROM sync_asset_uploads WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",
        params![scope.hub.to_string(), scope.scope.as_str(), operation.as_str(), custody.object.object.as_str()],
        |r| {
            let step = match (r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?, r.get::<_, Option<Vec<u8>>>(7)?) {
                (Some(scope), Some(source), Some(step)) => Some((scope, source, step)),
                _ => None,
            };
            Ok(Saved {
                kind: r.get(0)?,
                holding: r.get(1)?,
                purpose: r.get(2)?,
                cancelled: r.get(3)?,
                consumed: r.get(4)?,
                step,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}
fn step_columns(step: &StepKey) -> (String, String, Vec<u8>) {
    (step.scope.scope.as_str().to_owned(), step.source.as_str().to_owned(), step.revision.0.to_be_bytes().to_vec())
}

/// Creates or confirms an open intent. A cancelled or consumed intent refuses,
/// and a source intent also requires its step to be active with this operation
/// and to have bound this destination.
pub(super) fn open(db: &Connection, custody: &Custody, operation: &OperationId, owner: Owner<'_>) -> Result<()> {
    let scope = &custody.object.scope;
    if let Owner::Source(step) = owner {
        source_step_open(db, &scope.hub, step, operation, custody)?;
    }
    let holding = serde_json::to_vec(&custody.holding).map_err(|e| sql::invalid(e.to_string()))?;
    let purpose = serde_json::to_vec(&custody.purpose).map_err(|e| sql::invalid(e.to_string()))?;
    let expected_step = match owner {
        Owner::Replica => None,
        Owner::Source(step) => Some(step_columns(step)),
    };
    if let Some(found) = saved(db, custody, operation)? {
        if found.kind != custody.kind.registration().id
            || found.holding != holding
            || found.purpose != purpose
            || found.step != expected_step
        {
            return Err(sql::invalid("upload operation belongs to another kind, holding period or owner"));
        }
        if found.cancelled || found.consumed {
            return Err(sql::invalid("upload operation was cancelled or already consumed"));
        }
        return Ok(());
    }
    let (source_scope, source, source_step) = match expected_step {
        Some((scope, source, step)) => (Some(scope), Some(source), Some(step)),
        None => (None, None, None),
    };
    db.execute(
        "INSERT INTO sync_asset_uploads(hub,scope,operation,object,kind,holding,purpose,cancelled,source_scope,source,source_step,consumed) VALUES(?1,?2,?3,?4,?5,?6,?7,0,?8,?9,?10,0)",
        params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            operation.as_str(),
            custody.object.object.as_str(),
            custody.kind.registration().id,
            holding,
            purpose,
            source_scope,
            source,
            source_step
        ],
    )?;
    Ok(())
}
fn source_step_open(
    db: &Connection,
    hub: &HubId,
    step: &StepKey,
    operation: &OperationId,
    custody: &Custody,
) -> Result<()> {
    if source_step_active(db, hub, step, operation, custody)? {
        Ok(())
    } else {
        Err(sql::invalid("the source step or its bound destination is no longer active"))
    }
}
fn source_step_active(
    db: &Connection,
    hub: &HubId,
    step: &StepKey,
    operation: &OperationId,
    custody: &Custody,
) -> Result<bool> {
    let revision = step.revision.0.to_be_bytes();
    let active: bool = db
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_source_steps WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4 AND operation=?5 AND state='active')")?
        .query_row(
            params![hub.to_string(), step.scope.scope.as_str(), step.source.as_str(), revision.as_slice(), operation.as_str()],
            |row| row.get(0),
        )?;
    if !active {
        return Ok(false);
    }
    let bound: bool = db
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_source_step_targets WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4 AND target_scope=?5 AND object=?6 AND kind=?7 AND active=1)")?
        .query_row(
            params![
                hub.to_string(),
                step.scope.scope.as_str(),
                step.source.as_str(),
                revision.as_slice(),
                custody.object.scope.scope.as_str(),
                custody.object.object.as_str(),
                custody.kind.registration().id
            ],
            |row| row.get(0),
        )?;
    Ok(bound)
}

/// Records a cancelled intent before any pin exists, so a late put cannot stage.
fn fence(db: &Connection, custody: &Custody, operation: &OperationId) -> Result<()> {
    let scope = &custody.object.scope;
    let holding = serde_json::to_vec(&custody.holding).map_err(|e| sql::invalid(e.to_string()))?;
    let purpose = serde_json::to_vec(&custody.purpose).map_err(|e| sql::invalid(e.to_string()))?;
    db.execute(
        "INSERT INTO sync_asset_uploads(hub,scope,operation,object,kind,holding,purpose,cancelled) VALUES(?1,?2,?3,?4,?5,?6,?7,1)",
        params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            operation.as_str(),
            custody.object.object.as_str(),
            custody.kind.registration().id,
            holding,
            purpose
        ],
    )?;
    Ok(())
}

/// Cancelling an owned operation remains valid after its grant has advanced or
/// disappeared. The persisted holder, object and kind still have to match. A
/// source step's uploads are cancelled only through that step.
pub(super) fn cancel(
    db: &Connection,
    holder: &crate::cursor::Holder,
    custody: &Custody,
    operation: &OperationId,
) -> Result<()> {
    super::pins::same_holder(holder, custody)?;
    let scope = &custody.object.scope;
    let Some(found) = saved(db, custody, operation)? else {
        return fence(db, custody, operation);
    };
    let period: crate::cursor::HoldingPeriod =
        serde_json::from_slice(&found.holding).map_err(|e| sql::invalid(e.to_string()))?;
    if found.kind != custody.kind.registration().id || &period.holder != holder || period.scope != *scope {
        return Err(sql::invalid("upload cancellation belongs to another object kind or holder"));
    }
    if found.step.is_some() {
        return Err(sql::invalid("a source upload is cancelled through its step"));
    }
    db.execute(
        "UPDATE sync_asset_uploads SET cancelled=1 WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",
        params![scope.hub.to_string(), scope.scope.as_str(), operation.as_str(), custody.object.object.as_str()],
    )?;
    super::supply::cancelled_upload(db, scope, operation, &custody.object.object)
}

/// Cancels every open intent of a step; its pins are released separately in pages.
pub(crate) fn cancel_step(db: &Connection, hub: &HubId, step: &StepKey) -> Result<()> {
    db.prepare_cached("UPDATE sync_asset_uploads SET cancelled=1 WHERE hub=?1 AND source_scope=?2 AND source=?3 AND source_step=?4 AND consumed=0")?
        .execute(params![hub.to_string(), step.scope.scope.as_str(), step.source.as_str(), step.revision.0.to_be_bytes().as_slice()])?;
    let mut query=db.prepare_cached("SELECT scope,operation,object FROM sync_asset_uploads WHERE hub=?1 AND source_scope=?2 AND source=?3 AND source_step=?4 AND cancelled=1 LIMIT 256")?;
    let scopes = query
        .query_map(
            params![
                hub.to_string(),
                step.scope.scope.as_str(),
                step.source.as_str(),
                step.revision.0.to_be_bytes().as_slice()
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
        )?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    for (scope, operation, object) in scopes {
        let scope =
            ScopeKey { hub: hub.clone(), scope: crate::identity::ScopeId::try_from(scope).map_err(sql::invalid)? };
        let operation = OperationId::try_from(operation).map_err(sql::invalid)?;
        let object = crate::identity::ObjectId::try_from(object).map_err(sql::invalid)?;
        super::supply::cancelled_upload(db, &scope, &operation, &object)?;
    }
    Ok(())
}

/// Marks every open intent of a committed step consumed; none can stage again.
pub(crate) fn consume_step(db: &Connection, hub: &HubId, step: &StepKey) -> Result<()> {
    db.prepare_cached("UPDATE sync_asset_uploads SET consumed=1 WHERE hub=?1 AND source_scope=?2 AND source=?3 AND source_step=?4 AND cancelled=0")?
        .execute(params![hub.to_string(), step.scope.scope.as_str(), step.source.as_str(), step.revision.0.to_be_bytes().as_slice()])?;
    Ok(())
}

/// Releases up to `max` pins staged by a step that was cancelled or consumed.
pub(crate) fn release_step(db: &Connection, hub: &HubId, step: &StepKey, max: u32) -> Result<u32> {
    let rows = db
        .prepare_cached("SELECT p.scope,p.operation,p.object,p.hash,p.root_kind FROM sync_asset_uploads u JOIN sync_asset_pins p ON p.hub=u.hub AND p.scope=u.scope AND p.operation=u.operation AND p.object=u.object WHERE u.hub=?1 AND u.source_scope=?2 AND u.source=?3 AND u.source_step=?4 AND (u.cancelled=1 OR u.consumed=1) AND p.phase!='released' LIMIT ?5")?
        .query_map(
            params![hub.to_string(), step.scope.scope.as_str(), step.source.as_str(), step.revision.0.to_be_bytes().as_slice(), max],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?)),
        )?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    for (scope, operation, object, hash, form) in &rows {
        retire(db, &hub.to_string(), scope, operation, object, hash, form)?;
    }
    Ok(rows.len() as u32)
}

fn retire(
    db: &Connection,
    hub: &str,
    scope: &str,
    operation: &str,
    object: &str,
    hash: &str,
    form: &str,
) -> Result<()> {
    let changed = db.execute("UPDATE sync_asset_pins SET phase='released' WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4 AND hash=?5 AND root_kind=?6 AND phase!='released'", params![hub, scope, operation, object, hash, form])?;
    if changed != 0 {
        advance_supply(db, hub, scope, operation, object)?;
    }
    db.execute("INSERT INTO sync_asset_gc(hub,scope,hash,root_kind,generation,next_due) VALUES(?1,?2,?3,?4,?5,0) ON CONFLICT(hub,scope,hash,root_kind) DO NOTHING", params![hub, scope, hash, form, 1_u64.to_be_bytes().as_slice()])?;
    Ok(())
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
    let hub = scope.hub.to_string();
    for (operation, object, hash, form) in &rows {
        retire(db, &hub, scope.scope.as_str(), operation, object, hash, form)?;
    }
    Ok(rows.len() as u32)
}
