//! Private holds transfer a verified basis into the lifetime of one check.
use super::{
    model::{Request, SupplyOwner, SupplyTicket, decode, encode},
    rows::Check,
};
use crate::{
    blocks::{access, pins},
    cursor::{HoldingPeriod, Revision},
    identity::{Digest, RootLeaseId},
    manifest::ContentRef,
    store::{
        access::HoldingUse,
        roots::{RootLease, RootUse, SupplyBasis},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) fn bases(db: &Connection, check: &Check, previous: Option<&Check>, now: i64) -> Result<()> {
    let request = &check.request;
    let custody = request.custody()?;
    for basis in &request.bases {
        let root = match basis {
            SupplyBasis::Published { root } => {
                if !access::has_grant(db, &custody, root)? {
                    return Err(sql::invalid("declared basis has no published object grant"));
                }
                root
            }
            SupplyBasis::Held { lease } => {
                let transferred = match previous {
                    Some(old) if old.ticket.id == check.ticket.id => inherited(db, old, &lease.root)?,
                    _ => false,
                };
                if !transferred {
                    validate_lease(db, request, lease, now)?;
                }
                &lease.root
            }
        };
        hold(db, check, root)?;
    }
    Ok(())
}

fn validate_lease(db: &Connection, request: &Request, expected: &RootLease, now: i64) -> Result<()> {
    let scope = &request.object.scope;
    if expected.object != request.object
        || expected.kind != request.kind
        || expected.holding.holder != request.holding.holder
        || expected.holding.scope != request.holding.scope
        || expected.root.id().domain != *scope
        || !permitted_use(&request.owner, &expected.usage)
    {
        return Err(sql::invalid("basis lease differs from the requested object, source or use"));
    }
    let found = db.query_row(
        "SELECT object,kind,hash,root_kind,decoded_bytes,operation,generation,usage,holding,purpose,expires,released,supply_id FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND lease=?3",
        params![scope.hub.to_string(),scope.scope.as_str(),expected.id.as_str()],
        |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,Option<u32>>(4)?,row.get::<_,String>(5)?,sql::counter(row,6)?,row.get::<_,Vec<u8>>(7)?,row.get::<_,Vec<u8>>(8)?,row.get::<_,Vec<u8>>(9)?,row.get::<_,Option<i64>>(10)?,row.get::<_,bool>(11)?,row.get::<_,Option<String>>(12)?)),
    ).optional()?.ok_or_else(||sql::invalid("basis lease does not exist"))?;
    let (object, kind, hash, form, length, operation, generation, usage, holding, purpose, expires, released, internal) =
        found;
    let actual_usage: RootUse = decode(&usage)?;
    let actual_holding: HoldingPeriod = decode(&holding)?;
    let actual_purpose: HoldingUse = decode(&purpose)?;
    if object != request.object.object.as_str()
        || kind != request.kind
        || hash != expected.root.id().digest.as_str()
        || form != pins::shape(&expected.root).0
        || length != pins::shape(&expected.root).1
        || operation != expected.operation.as_str()
        || generation != expected.generation.0
        || actual_usage != expected.usage
        || actual_holding.holder != request.holding.holder
        || actual_holding.scope != request.holding.scope
        || actual_purpose != request.purpose
        || expires != expected.expires_at_millis
        || expires.is_some_and(|expires| expires <= now)
        || released
        || internal.is_some()
    {
        return Err(sql::invalid("basis lease has expired, been released or changed identity"));
    }
    if actual_holding != request.holding {
        return Err(fastctx_storage::Error::with_kind(
            fastctx_storage::ErrorKind::Busy,
            "basis lease is waiting for holding-period reconciliation",
        ));
    }
    Ok(())
}
fn inherited(db: &Connection, previous: &Check, root: &ContentRef) -> Result<bool> {
    let ticket = &previous.ticket;
    let scope = &ticket.scope;
    let id = identity(ticket, root)?;
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND lease=?3 AND supply_id=?4 AND supply_generation=?5 AND holding=?6 AND released=0)",
        params![scope.hub.to_string(),scope.scope.as_str(),id.as_str(),ticket.id.as_str(),ticket.generation.0.to_be_bytes().as_slice(),encode(&previous.request.holding)?],|row|row.get(0),
    )?)
}
fn permitted_use(owner: &SupplyOwner, usage: &RootUse) -> bool {
    match (owner, usage) {
        (SupplyOwner::Source { source: left }, RootUse::Checkpoint { source: right }) => left == right,
        (SupplyOwner::Use { usage: left }, right) => left == right && !matches!(right, RootUse::Timeline { .. }),
        _ => false,
    }
}

pub(super) fn grant(db: &Connection, check: &Check, root: &ContentRef) -> Result<bool> {
    if !access::has_grant(db, &check.request.custody()?, root)? {
        return Ok(false);
    }
    hold(db, check, root)?;
    Ok(true)
}

fn identity(ticket: &SupplyTicket, root: &ContentRef) -> Result<RootLeaseId> {
    let mut bytes = b"fastctx-sync/supply-protection/v1\0".to_vec();
    bytes.extend_from_slice(&ticket.id.bytes());
    bytes.extend_from_slice(&ticket.generation.0.to_be_bytes());
    bytes.extend(encode(root)?);
    Ok(RootLeaseId::from_bytes(Digest::of(&bytes).bytes()))
}
fn hold(db: &Connection, check: &Check, root: &ContentRef) -> Result<()> {
    let request = &check.request;
    if root.id().domain != request.object.scope {
        return Err(sql::invalid("basis protection crosses its object scope"));
    }
    let scope = &request.object.scope;
    let id = identity(&check.ticket, root)?;
    let usage =
        encode(&serde_json::json!({"use":"supply","check":check.ticket.id,"generation":check.ticket.generation}))?;
    let changed = db.execute(
        "INSERT INTO sync_root_leases(hub,scope,lease,object,kind,hash,root_kind,decoded_bytes,operation,generation,usage,holding,purpose,expires,released,supply_id,supply_generation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,NULL,0,?14,?15) ON CONFLICT(hub,scope,lease) DO NOTHING",
        params![scope.hub.to_string(),scope.scope.as_str(),id.as_str(),request.object.object.as_str(),request.kind,root.id().digest.as_str(),pins::shape(root).0,pins::shape(root).1,request.operation.as_str(),1_u64.to_be_bytes().as_slice(),usage,encode(&request.holding)?,encode(&request.purpose)?,check.ticket.id.as_str(),check.ticket.generation.0.to_be_bytes().as_slice()],
    )?;
    if changed == 0 {
        let valid:bool=db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND lease=?3 AND supply_id=?4 AND supply_generation=?5 AND released=0 AND holding=?6)",
            params![scope.hub.to_string(),scope.scope.as_str(),id.as_str(),check.ticket.id.as_str(),check.ticket.generation.0.to_be_bytes().as_slice(),encode(&request.holding)?],|row|row.get(0),
        )?;
        if !valid {
            return Err(sql::invalid("supply protection was invalidated"));
        }
    }
    Ok(())
}

pub(super) fn retire_page(db: &Connection, ticket: &SupplyTicket, max: u32) -> Result<u32> {
    if max == 0 || max > 64 {
        return Err(sql::invalid("supply cleanup needs one to 64 holds"));
    }
    let scope = &ticket.scope;
    let mut query = db.prepare_cached(
        "SELECT lease,hash,root_kind FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND supply_id=?3 AND supply_generation=?4 AND released=0 ORDER BY lease LIMIT ?5",
    )?;
    let rows = query
        .query_map(
            params![
                scope.hub.to_string(),
                scope.scope.as_str(),
                ticket.id.as_str(),
                ticket.generation.0.to_be_bytes().as_slice(),
                max
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
        )?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    for (lease, hash, form) in &rows {
        db.execute(
            "UPDATE sync_root_leases SET released=1 WHERE hub=?1 AND scope=?2 AND lease=?3",
            params![scope.hub.to_string(), scope.scope.as_str(), lease],
        )?;
        db.execute(
            "INSERT INTO sync_asset_gc(hub,scope,hash,root_kind,generation,next_due) VALUES(?1,?2,?3,?4,?5,0) ON CONFLICT(hub,scope,hash,root_kind) DO NOTHING",
            params![scope.hub.to_string(),scope.scope.as_str(),hash,form,Revision(1).0.to_be_bytes().as_slice()],
        )?;
    }
    Ok(rows.len() as u32)
}
