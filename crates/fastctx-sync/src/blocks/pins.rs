//! Pins bind staging and garbage collection to one object and holding period.

use crate::{
    cursor::{Holder, Revision},
    identity::OperationId,
    manifest::{ContentRef, Pin},
    store::{
        access::{AccessPolicy, Custody, Decision},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

/// Releasing local ownership requires identity, not a still-current grant.
pub(super) fn same_holder(holder: &Holder, custody: &Custody) -> Result<()> {
    if &custody.holding.holder != holder
        || custody.context.scope != custody.object.scope
        || custody.holding.scope != custody.object.scope
    {
        return Err(sql::invalid("release differs from the actual holder or object scope"));
    }
    Ok(())
}

pub(super) fn authorize(db: &Connection, holder: &Holder, policy: &dyn AccessPolicy, custody: &Custody) -> Result<()> {
    if &custody.holding.holder != holder
        || custody.context.scope != custody.object.scope
        || custody.holding.scope != custody.object.scope
        || custody.holding.acl_revision != custody.context.observed_acl
    {
        return Err(sql::invalid("content custody differs from the actual holder, object or ACL"));
    }
    let scope = &custody.object.scope;
    let current = db
        .query_row(
            "SELECT enabled,acl_revision,publication_epoch FROM sync_scopes WHERE hub=?1 AND scope=?2",
            params![scope.hub.to_string(), scope.scope.as_str()],
            |row| Ok((row.get::<_, bool>(0)?, sql::counter(row, 1)?, sql::counter(row, 2)?)),
        )
        .optional()?;
    if current != Some((true, custody.holding.acl_revision.0, custody.holding.publication_epoch.0)) {
        return Err(sql::invalid("content holding period is no longer current"));
    }
    let deleted: bool = db
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3)")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), custody.object.object.as_str()], |r| {
            r.get(0)
        })?;
    if deleted {
        return Err(sql::invalid("deleted objects cannot publish or retain new content"));
    }
    if !matches!(policy.can_keep(&custody.context,custody.kind,&custody.object,&custody.purpose),Decision::Allowed {revision} if revision==custody.context.observed_acl)
    {
        return Err(sql::invalid("current object custody does not permit content storage"));
    }
    Ok(())
}

pub(super) fn authorize_read(
    db: &Connection,
    holder: &Holder,
    policy: &dyn AccessPolicy,
    custody: &Custody,
) -> Result<()> {
    authorize(db, holder, policy, custody)?;
    if !matches!(policy.can_read(&custody.context, custody.kind), Decision::Allowed { revision } if revision == custody.context.observed_acl)
        || !matches!(policy.can_read_object(&custody.context, custody.kind, &custody.object), Decision::Allowed { revision } if revision == custody.context.observed_acl)
    {
        return Err(sql::invalid("the current reader cannot read this object kind"));
    }
    Ok(())
}

pub(super) fn shape(root: &ContentRef) -> (&'static str, Option<u32>) {
    match root {
        ContentRef::Single { decoded_bytes, .. } => ("block", Some(*decoded_bytes)),
        ContentRef::Manifest { .. } => ("manifest", None),
    }
}

pub(super) fn stage(db: &Connection, custody: &Custody, operation: OperationId, root: ContentRef) -> Result<Pin> {
    root.validate().map_err(sql::invalid)?;
    super::uploads::intent(db, custody, &operation, false)?;
    if root.id().domain != custody.object.scope {
        return Err(sql::invalid("pin content belongs to another scope"));
    }
    let scope = &custody.object.scope;
    let (root_kind, decoded_bytes) = shape(&root);
    let holding = serde_json::to_vec(&custody.holding).map_err(|error| sql::invalid(error.to_string()))?;
    let found=db.query_row(
        "SELECT kind,decoded_bytes,generation,phase,holding FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4 AND hash=?5 AND root_kind=?6",
        params![scope.hub.to_string(),scope.scope.as_str(),operation.as_str(),custody.object.object.as_str(),root.id().digest.as_str(),root_kind],
        |row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<u32>>(1)?,sql::counter(row,2)?,row.get::<_,String>(3)?,row.get::<_,Vec<u8>>(4)?))
    ).optional()?;
    let generation = if let Some((kind, length, generation, phase, period)) = found {
        if kind != custody.kind.registration().id || length != decoded_bytes || period != holding || phase == "released"
        {
            return Err(sql::invalid("a staging operation cannot change its object, content form or holding period"));
        }
        Revision(generation)
    } else {
        let generation = Revision(1);
        db.execute(
            "INSERT INTO sync_asset_pins(hub,scope,operation,object,kind,hash,root_kind,decoded_bytes,generation,phase,holding) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'staging',?10)",
            params![scope.hub.to_string(),scope.scope.as_str(),operation.as_str(),custody.object.object.as_str(),custody.kind.registration().id,root.id().digest.as_str(),root_kind,decoded_bytes,generation.0.to_be_bytes().as_slice(),holding]
        )?;
        generation
    };
    Ok(Pin {
        operation,
        object: custody.object.clone(),
        kind: custody.kind.registration().id.into(),
        root,
        generation,
        holding: custody.holding.clone(),
    })
}

pub(super) fn check(db: &Connection, custody: &Custody, pin: &Pin) -> Result<String> {
    if pin.object != custody.object
        || pin.kind != custody.kind.registration().id
        || pin.holding != custody.holding
        || pin.root.id().domain != pin.object.scope
    {
        return Err(sql::invalid("pin does not belong to this object and custody"));
    }
    let scope = &pin.object.scope;
    let (root_kind, decoded_bytes) = shape(&pin.root);
    let holding = serde_json::to_vec(&pin.holding).map_err(|error| sql::invalid(error.to_string()))?;
    let found=db.query_row(
        "SELECT phase FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4 AND kind=?5 AND hash=?6 AND root_kind=?7 AND decoded_bytes IS ?8 AND generation=?9 AND holding=?10",
        params![scope.hub.to_string(),scope.scope.as_str(),pin.operation.as_str(),pin.object.object.as_str(),pin.kind,pin.root.id().digest.as_str(),root_kind,decoded_bytes,pin.generation.0.to_be_bytes().as_slice(),holding],
        |row|row.get::<_,String>(0)
    ).optional()?;
    found.ok_or_else(|| sql::invalid("pin no longer matches the persisted staging intent"))
}

pub(super) fn ready(db: &Connection, custody: &Custody, pin: &Pin) -> Result<()> {
    super::uploads::intent(db, custody, &pin.operation, false)?;
    if check(db, custody, pin)? == "released" {
        return Err(sql::invalid("a released pin cannot publish content"));
    }
    phase(db, pin, "ready")
}

pub(super) fn release(db: &Connection, custody: &Custody, pin: &Pin) -> Result<()> {
    if check(db, custody, pin)? == "released" {
        return Ok(());
    }
    phase(db, pin, "released")?;
    let scope = &pin.object.scope;
    db.execute(
        "INSERT INTO sync_asset_gc(hub,scope,hash,root_kind,generation,next_due) VALUES(?1,?2,?3,?4,?5,0) ON CONFLICT(hub,scope,hash,root_kind) DO NOTHING",
        params![scope.hub.to_string(),scope.scope.as_str(),pin.root.id().digest.as_str(),shape(&pin.root).0,1_u64.to_be_bytes().as_slice()]
    )?;
    Ok(())
}

fn phase(db: &Connection, pin: &Pin, phase: &str) -> Result<()> {
    let scope = &pin.object.scope;
    db.execute(
        "UPDATE sync_asset_pins SET phase=?1 WHERE hub=?2 AND scope=?3 AND operation=?4 AND object=?5 AND hash=?6 AND root_kind=?7",
        params![phase,scope.hub.to_string(),scope.scope.as_str(),pin.operation.as_str(),pin.object.object.as_str(),pin.root.id().digest.as_str(),shape(&pin.root).0]
    )?;
    Ok(())
}
