//! SQL-only usage holds share the writer with source checkpoints and runtime plans.
use super::{pins, rows};
use crate::{
    cursor::{Completeness, Revision},
    identity::{OperationId, RootLeaseId, ScopeKey},
    store::{
        access::{Custody, HoldingUse},
        roots::{AcquireRoot, RootAcquired, RootLease},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

mod current;
mod saved;
pub(super) use current::retained;
mod sweep;
pub(super) use sweep::{expire, retire_scope};

/// Idempotent lookup precedes supply validation: an acquired use outlives the
/// staging operation that originally supplied it.
pub(super) fn existing(
    db: &Connection,
    operation: &OperationId,
    request: &AcquireRoot,
    now: i64,
) -> Result<Option<RootAcquired>> {
    request.validate().map_err(|error| sql::invalid(error.to_string()))?;
    if request.expires_at_millis.is_some_and(|expiry| expiry <= now) {
        return Err(sql::invalid("root usage has already expired"));
    }
    let custody = &request.custody;
    let scope = &custody.object.scope;
    let lease = RootLease {
        id: request.lease.clone(),
        operation: operation.clone(),
        object: custody.object.clone(),
        kind: custody.kind.registration().id.into(),
        root: request.root.clone(),
        usage: request.usage.clone(),
        holding: custody.holding.clone(),
        generation: Revision(1),
        expires_at_millis: request.expires_at_millis,
    };
    let encoded = saved::encode(&lease, &custody.purpose)?;
    if let Some(previous) = saved::get(db, scope, &request.lease)? {
        if previous.released {
            return Err(sql::invalid("a released usage identity cannot be acquired again"));
        }
        previous.check(&encoded)?;
        return if matches!(request.usage, crate::store::roots::RootUse::Timeline { .. })
            || rows::complete(db, &request.root)?
        {
            Ok(Some(RootAcquired::Held { lease: Box::new(lease) }))
        } else {
            Ok(Some(RootAcquired::Pending { completeness: Completeness::MetadataOnly }))
        };
    }
    Ok(None)
}

/// The supply writer has rechecked this root in the enclosing transaction.
pub(super) fn insert(db: &Connection, operation: &OperationId, request: &AcquireRoot) -> Result<RootLease> {
    let custody = &request.custody;
    let scope = &custody.object.scope;
    let lease = RootLease {
        id: request.lease.clone(),
        operation: operation.clone(),
        object: custody.object.clone(),
        kind: custody.kind.registration().id.into(),
        root: request.root.clone(),
        usage: request.usage.clone(),
        holding: custody.holding.clone(),
        generation: Revision(1),
        expires_at_millis: request.expires_at_millis,
    };
    let encoded = saved::encode(&lease, &custody.purpose)?;
    let (form, length) = pins::shape(&request.root);
    db.execute(
        "INSERT INTO sync_root_leases(hub,scope,lease,object,kind,hash,root_kind,decoded_bytes,operation,generation,usage,holding,purpose,expires,released) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,0)",
        params![scope.hub.to_string(),scope.scope.as_str(),lease.id.as_str(),lease.object.object.as_str(),lease.kind,lease.root.id().digest.as_str(),form,length,operation.as_str(),lease.generation.0.to_be_bytes().as_slice(),encoded.usage,encoded.holding,encoded.purpose,lease.expires_at_millis],
    )?;
    Ok(lease)
}

pub(super) fn release(
    db: &Connection,
    holder: &crate::cursor::Holder,
    custody: &Custody,
    lease: &RootLease,
) -> Result<()> {
    pins::same_holder(holder, custody)?;
    if lease.object != custody.object
        || lease.kind != custody.kind.registration().id
        || &lease.holding.holder != holder
        || lease.holding.scope != custody.object.scope
        || lease.root.id().domain != custody.object.scope
    {
        return Err(sql::invalid("root usage belongs to another object, kind or holding period"));
    }
    let scope = &custody.object.scope;
    let encoded = saved::encode(lease, &custody.purpose)?;
    let previous = saved::get(db, scope, &lease.id)?.ok_or_else(|| sql::invalid("root usage does not exist"))?;
    previous.check_release(&encoded, holder)?;
    if !previous.released {
        retire(db, scope, &lease.id, &previous.hash, &previous.form)?;
    }
    Ok(())
}

/// Releases a use known only by its identity, such as a reader's fence. The
/// persisted holder must match; an unknown or released use needs nothing.
pub(super) fn release_id(
    db: &Connection,
    holder: &crate::cursor::Holder,
    scope: &ScopeKey,
    id: &RootLeaseId,
) -> Result<()> {
    let Some(previous) = saved::get(db, scope, id)? else {
        return Ok(());
    };
    if &previous.holder()? != holder {
        return Err(sql::invalid("root usage is owned by another holder"));
    }
    if !previous.released {
        retire(db, scope, id, &previous.hash, &previous.form)?;
    }
    Ok(())
}

pub(super) fn retire(db: &Connection, scope: &ScopeKey, id: &RootLeaseId, hash: &str, form: &str) -> Result<()> {
    super::supply::invalidate_hold(db, scope, id)?;
    db.execute(
        "UPDATE sync_root_leases SET released=1 WHERE hub=?1 AND scope=?2 AND lease=?3",
        params![scope.hub.to_string(), scope.scope.as_str(), id.as_str()],
    )?;
    db.execute(
        "INSERT INTO sync_asset_gc(hub,scope,hash,root_kind,generation,next_due) VALUES(?1,?2,?3,?4,?5,0) ON CONFLICT(hub,scope,hash,root_kind) DO NOTHING",
        params![scope.hub.to_string(),scope.scope.as_str(),hash,form,1_u64.to_be_bytes().as_slice()],
    )?;
    Ok(())
}
