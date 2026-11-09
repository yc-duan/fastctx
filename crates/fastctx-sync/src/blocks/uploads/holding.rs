//! A period sweep retires pins through the same invalidation and GC path as release.
use super::*;
use crate::cursor::HoldingPeriod;
#[derive(serde::Serialize, serde::Deserialize)]
pub(in crate::blocks) struct PinCursor {
    operation: String,
    object: String,
    kind: String,
    hash: String,
    form: String,
}

pub(in crate::blocks) fn retire_old(
    db: &Connection,
    current: &HoldingPeriod,
    after: Option<&PinCursor>,
    max: u32,
) -> Result<Option<PinCursor>> {
    let scope = &current.scope;
    let rows=db.prepare_cached("SELECT operation,object,kind,hash,root_kind,holding FROM sync_asset_pins INDEXED BY sync_asset_pins_cleanup WHERE hub=?1 AND scope=?2 AND phase!='released' AND (operation,object,kind,hash,root_kind)>(?3,?4,?5,?6,?7) ORDER BY operation,object,kind,hash,root_kind LIMIT ?8")?.query_map(
        params![scope.hub.to_string(),scope.scope.as_str(),after.map_or("",|c|c.operation.as_str()),after.map_or("",|c|c.object.as_str()),after.map_or("",|c|c.kind.as_str()),after.map_or("",|c|c.hash.as_str()),after.map_or("",|c|c.form.as_str()),max],
        |row|Ok((PinCursor{operation:row.get(0)?,object:row.get(1)?,kind:row.get(2)?,hash:row.get(3)?,form:row.get(4)?},row.get::<_,Vec<u8>>(5)?)),
    )?.collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    let full = rows.len() == max as usize;
    let mut last = None;
    for (key, holding) in rows {
        let old: HoldingPeriod = serde_json::from_slice(&holding).map_err(|error| sql::invalid(error.to_string()))?;
        if old.holder != current.holder || old.scope != *scope {
            return Err(sql::invalid("pin belongs to another holder or scope"));
        }
        if old != *current {
            retire(
                db,
                &scope.hub.to_string(),
                scope.scope.as_str(),
                &key.operation,
                &key.object,
                &key.hash,
                &key.form,
            )?;
        }
        last = Some(key);
    }
    Ok(if full { last } else { None })
}
