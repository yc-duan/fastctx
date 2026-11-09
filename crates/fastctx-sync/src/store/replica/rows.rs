//! Receiver coordinates are fenced by custody; peer ACKs do not share this table.
use crate::{
    cursor::{HoldingPeriod, LocalSequence, ScopeCursor},
    identity::ObjectKey,
    registry::REGISTRY,
    store::{
        access::{AccessContext, Custody, HoldingUse, Principal},
        replica::ReplicaContext,
        scopes, sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use serde::{Serialize, de::DeserializeOwned};

pub(super) fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| sql::invalid(e.to_string()))
}
pub(super) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| sql::invalid(e.to_string()))
}
pub(super) fn current(db: &Connection, holding: &HoldingPeriod) -> Result<()> {
    let state = scopes::current(db, &holding.scope)?.ok_or_else(|| sql::invalid("replica scope is not installed"))?;
    if !state.enabled
        || state.acl_revision != holding.acl_revision
        || state.publication_epoch != holding.publication_epoch
    {
        return Err(sql::invalid("replica holding period is no longer current"));
    }
    Ok(())
}
pub(super) fn custody(context: &ReplicaContext, object: &ObjectKey, kind: &str) -> Result<Custody> {
    if object.scope != context.holding.scope {
        return Err(sql::invalid("replica object belongs to another scope"));
    }
    Ok(Custody {
        context: AccessContext {
            principal: Principal::LocalService,
            scope: object.scope.clone(),
            observed_acl: context.holding.acl_revision,
        },
        kind: REGISTRY.asset(kind).map_err(sql::invalid)?.registered(),
        object: object.clone(),
        purpose: context.source_uses.get(object).cloned().unwrap_or(HoldingUse::Replica),
        holding: context.holding.clone(),
    })
}
pub(super) fn cursor(db: &Connection, holding: &HoldingPeriod) -> Result<Option<(ScopeCursor, LocalSequence)>> {
    let found = db
        .prepare_cached("SELECT holding,cursor,local FROM sync_replica_scopes WHERE hub=?1 AND scope=?2")?
        .query_row(params![holding.scope.hub.to_string(), holding.scope.scope.as_str()], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?, sql::counter(r, 2)?))
        })
        .optional()?;
    found
        .map(|(period, bytes, local)| {
            if decode::<HoldingPeriod>(&period)? != *holding {
                return Err(sql::invalid("the replica scope needs a holding/view transition"));
            }
            Ok((decode(&bytes)?, LocalSequence(local)))
        })
        .transpose()
}
