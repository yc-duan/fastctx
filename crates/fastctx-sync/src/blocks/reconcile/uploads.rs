//! Close old upload intents before retiring their individual staged pins.
use super::*;
use crate::{
    identity::{ObjectId, OperationId, ScopeId, SourceId},
    store::source_step::StepKey,
};
use state::{Progress, UploadKey};

pub(super) fn page(db: &Connection, current: &HoldingPeriod, after: Option<UploadKey>) -> SqlResult<Progress> {
    let scope = &current.scope;
    let rows=db.prepare_cached("SELECT operation,object,holding,source_scope,source,source_step FROM sync_asset_uploads INDEXED BY sync_asset_uploads_open WHERE hub=?1 AND scope=?2 AND cancelled=0 AND consumed=0 AND (operation,object)>(?3,?4) ORDER BY operation,object LIMIT 64")?.query_map(
        params![scope.hub.to_string(),scope.scope.as_str(),after.as_ref().map_or("",|key|key.operation.as_str()),after.as_ref().map_or("",|key|key.object.as_str())],
        |row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,Option<Vec<u8>>>(5)?)),
    )?.collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    let full = rows.len() == 64;
    let mut last = after;
    for (operation, object, holding, source_scope, source, step) in rows {
        let key = UploadKey {
            operation: OperationId::try_from(operation).map_err(sql::invalid)?,
            object: ObjectId::try_from(object).map_err(sql::invalid)?,
        };
        let old: HoldingPeriod = state::decode(&holding)?;
        if old.holder != current.holder || old.scope != *scope {
            return Err(sql::invalid("upload intent belongs to another holder or scope"));
        }
        last = Some(key.clone());
        if old == *current {
            continue;
        }
        if let (Some(source_scope), Some(source), Some(step)) = (source_scope, source, step) {
            let step = StepKey {
                scope: ScopeKey {
                    hub: scope.hub.clone(),
                    scope: ScopeId::try_from(source_scope).map_err(sql::invalid)?,
                },
                source: SourceId::try_from(source).map_err(sql::invalid)?,
                revision: Revision(u64::from_be_bytes(
                    step.try_into().map_err(|_| sql::invalid("invalid upload step revision"))?,
                )),
            };
            super::super::steps::retire(db, &scope.hub, &step)?;
            crate::scheduler::source_cleanup(db, &step)?;
            // A step can fan out to 256 targets. Retire only one such step in
            // this transaction; its independent task survives another scope advance.
            return Ok(Progress::Uploads { after: last });
        }
        db.execute(
            "UPDATE sync_asset_uploads SET cancelled=1 WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",
            params![scope.hub.to_string(), scope.scope.as_str(), key.operation.as_str(), key.object.as_str()],
        )?;
        super::super::supply::cancelled_upload(db, scope, &key.operation, &key.object)?;
    }
    Ok(if full { Progress::Uploads { after: last } } else { Progress::Pins { after: None } })
}
