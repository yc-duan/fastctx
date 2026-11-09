//! A formal use keeps its immutable identity while custody moves to a new fence.
use super::*;
use crate::{
    identity::{ObjectId, ObjectKey, RootLeaseId},
    registry::REGISTRY,
    store::access::{AccessContext, AccessPolicy, Decision, HoldingUse, Principal},
};

pub(super) fn page(
    db: &Connection,
    current: &HoldingPeriod,
    enabled: bool,
    policy: &dyn AccessPolicy,
    after: Option<&RootLeaseId>,
    now: i64,
) -> SqlResult<Option<RootLeaseId>> {
    let scope = &current.scope;
    let rows=db.prepare_cached("SELECT lease,object,kind,hash,root_kind,holding,purpose,expires FROM sync_root_leases INDEXED BY sync_root_leases_formal WHERE hub=?1 AND scope=?2 AND released=0 AND supply_id IS NULL AND lease>?3 ORDER BY lease LIMIT 64")?.query_map(
        params![scope.hub.to_string(),scope.scope.as_str(),after.map_or("",RootLeaseId::as_str)],
        |row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,Vec<u8>>(5)?,row.get::<_,Vec<u8>>(6)?,row.get::<_,Option<i64>>(7)?)),
    )?.collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    let full = rows.len() == 64;
    let mut last = None;
    for (lease, object, kind, hash, form, holding, purpose, expires) in rows {
        let id = RootLeaseId::try_from(lease).map_err(sql::invalid)?;
        let old: HoldingPeriod = state::decode(&holding)?;
        if old.holder != current.holder || old.scope != *scope {
            return Err(sql::invalid("root use belongs to another holder or scope"));
        }
        if old == *current {
            last = Some(id);
            continue;
        }
        let object = ObjectKey { scope: scope.clone(), object: ObjectId::try_from(object).map_err(sql::invalid)? };
        let purpose: HoldingUse = state::decode(&purpose)?;
        let keep = if !enabled
            || expires.is_some_and(|end| end <= now)
            || db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3)",
                params![scope.hub.to_string(), scope.scope.as_str(), object.object.as_str()],
                |row| row.get::<_, bool>(0),
            )? {
            false
        } else {
            let context = AccessContext {
                principal: Principal::LocalService,
                scope: scope.clone(),
                observed_acl: current.acl_revision,
            };
            let kind = match REGISTRY.entries().iter().find(|entry| entry.id == kind).map(|entry| entry.class) {
                Some(crate::registry::DataClass::Asset) => REGISTRY.asset(&kind).map_err(sql::invalid)?.registered(),
                Some(crate::registry::DataClass::Derived) => {
                    REGISTRY.projection(&kind).map_err(sql::invalid)?.registered()
                }
                _ => return Err(sql::invalid("root use has no registered content kind")),
            };
            match policy.can_keep(&context, kind, &object, &purpose) {
                Decision::Allowed { revision } if revision == current.acl_revision => true,
                Decision::Denied { revision, .. } if revision == current.acl_revision => false,
                _ => {
                    return Err(fastctx_storage::Error::with_kind(
                        fastctx_storage::ErrorKind::Busy,
                        "custody policy has not reached the new scope fence",
                    ));
                }
            }
        };
        if keep {
            db.execute(
                "UPDATE sync_root_leases SET holding=?4 WHERE hub=?1 AND scope=?2 AND lease=?3 AND released=0",
                params![scope.hub.to_string(), scope.scope.as_str(), id.as_str(), state::encode(current)?],
            )?;
        } else {
            super::super::root_holds::retire(db, scope, &id, &hash, &form)?;
        }
        last = Some(id);
    }
    Ok(if full { last } else { None })
}
