//! Index validation cannot borrow another object's already stored payload bytes.
use super::SqliteBlocks;
use crate::{
    blocks::pins,
    cursor::HoldingPeriod,
    identity::{ObjectId, ObjectKey},
    manifest::{ContentBlock, ManifestRoot},
    registry::REGISTRY,
    store::{
        access::{AccessContext, Custody, Principal},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, params},
};

pub(super) fn current(
    db: &Connection,
    store: &SqliteBlocks,
    holding: &HoldingPeriod,
    root: &ManifestRoot,
    payload: &ContentBlock,
    after: Option<&str>,
) -> Result<(Option<Custody>, Option<String>)> {
    let (after_operation, after_object) = after.and_then(|value| value.split_once('/')).unwrap_or(("", ""));
    let period = serde_json::to_vec(holding).map_err(|e| sql::invalid(e.to_string()))?;
    let mut query=db.prepare_cached("SELECT p.operation,p.object,p.kind,u.purpose FROM sync_asset_pins p JOIN sync_asset_uploads u USING(hub,scope,operation,object) WHERE p.hub=?1 AND p.scope=?2 AND p.hash=?3 AND p.root_kind='manifest' AND p.holding=?4 AND (p.operation,p.object)>(?5,?6) ORDER BY p.operation,p.object LIMIT 65")?;
    let rows = query
        .query_map(
            params![
                holding.scope.hub.to_string(),
                holding.scope.scope.as_str(),
                root.0.digest.as_str(),
                period,
                after_operation,
                after_object
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Vec<u8>>(3)?)),
        )?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    let next = (rows.len() > 64).then(|| format!("{}/{}", rows[63].0, rows[63].1));
    for (_, object, kind, purpose) in rows.into_iter().take(64) {
        let custody = Custody {
            context: AccessContext {
                principal: Principal::LocalService,
                scope: holding.scope.clone(),
                observed_acl: holding.acl_revision,
            },
            object: ObjectKey {
                scope: holding.scope.clone(),
                object: ObjectId::try_from(object).map_err(sql::invalid)?,
            },
            kind: REGISTRY.asset(&kind).map_err(sql::invalid)?.registered(),
            purpose: serde_json::from_slice(&purpose).map_err(|e| sql::invalid(e.to_string()))?,
            holding: holding.clone(),
        };
        if pins::authorize(db, &store.holder, store.policy.as_ref(), &custody).is_err() {
            continue;
        }
        // Even a released block pin proves its bytes were supplied and checked
        // for this object. It does not grant an interactive reader access.
        let known:bool=db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='block' AND decoded_bytes=?6 AND holding=?7) OR EXISTS(SELECT 1 FROM sync_asset_access WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='block' AND decoded_bytes=?6)")?.query_row(params![holding.scope.hub.to_string(),holding.scope.scope.as_str(),custody.object.object.as_str(),kind,payload.id.digest.as_str(),payload.decoded_bytes,period],|r|r.get(0))?;
        if known {
            return Ok((Some(custody), None));
        }
    }
    Ok((None, next))
}
