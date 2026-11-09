//! Object-specific reachability is derived only from authenticated published roots.
use super::{manifest::Reference, pins};
use crate::{
    identity::ObjectKey,
    manifest::{ContentRef, ManifestRoot},
    store::{access::Custody, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

/// A verified asset/source mutation calls this inside its root-reference commit.
/// Staging a caller-authored manifest alone never invokes this entry point.
pub(crate) fn grant_root(
    db: &Connection,
    object: &ObjectKey,
    kind: &str,
    root: &ContentRef,
    holding: &crate::cursor::HoldingPeriod,
) -> Result<()> {
    if root.id().domain != object.scope || holding.scope != object.scope {
        return Err(sql::invalid("published root differs from object scope"));
    }
    let (form, length) = pins::shape(root);
    let changed = db.execute(
        "INSERT INTO sync_asset_access(hub,scope,object,kind,hash,root_kind,decoded_bytes,expanded) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(hub,scope,object,kind,hash,root_kind) DO NOTHING",
        params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),kind,root.id().digest.as_str(),form,length,form=="block"]
    )?;
    if changed != 0 && form == "manifest" {
        crate::scheduler::content_changed(db, holding, format!("access:{}", root.id().digest.as_str()))?;
    }
    Ok(())
}

pub(super) fn has_grant(db: &Connection, custody: &Custody, root: &ContentRef) -> Result<bool> {
    let scope = &custody.object.scope;
    let (form, length) = pins::shape(root);
    if root.id().domain != *scope {
        return Ok(false);
    }
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_asset_access WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind=?6 AND decoded_bytes IS ?7)",
        params![scope.hub.to_string(),scope.scope.as_str(),custody.object.object.as_str(),custody.kind.registration().id,root.id().digest.as_str(),form,length],|row|row.get(0)
    ).map_err(Into::into)
}

pub(super) fn block_ref(db: &Connection, custody: &Custody, id: &crate::manifest::BlobId) -> Result<ContentRef> {
    if id.domain != custody.object.scope {
        return Err(sql::invalid("content belongs to another object scope"));
    }
    let period = serde_json::to_vec(&custody.holding).map_err(|error| sql::invalid(error.to_string()))?;
    let length = db.query_row(
        "SELECT decoded_bytes FROM sync_asset_access WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='block' UNION ALL SELECT decoded_bytes FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='block' AND phase='ready' AND holding=?6 LIMIT 1",
        params![id.domain.hub.to_string(), id.domain.scope.as_str(), custody.object.object.as_str(), custody.kind.registration().id, id.digest.as_str(), period], |row| row.get::<_,u32>(0)
    ).optional()?.ok_or_else(|| sql::invalid("no block reference is authorized for this object and kind"))?;
    let reference = ContentRef::Single { block: id.clone(), decoded_bytes: length };
    require(db, custody, &reference)?;
    Ok(reference)
}

pub(super) fn require(db: &Connection, custody: &Custody, root: &ContentRef) -> Result<()> {
    let scope = &custody.object.scope;
    let deleted = db
        .query_row(
            "SELECT 1 FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3",
            params![scope.hub.to_string(), scope.scope.as_str(), custody.object.object.as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if deleted {
        return Err(sql::invalid("deleted objects cannot read staged or historical content"));
    }
    if has_grant(db, custody, root)? {
        return Ok(());
    }
    let holding = serde_json::to_vec(&custody.holding).map_err(|error| sql::invalid(error.to_string()))?;
    let (form, length) = pins::shape(root);
    let supplied:bool=db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind=?6 AND decoded_bytes IS ?7 AND holding=?8 AND phase='ready')",
        params![scope.hub.to_string(),scope.scope.as_str(),custody.object.object.as_str(),custody.kind.registration().id,root.id().digest.as_str(),form,length,holding],|row|row.get(0)
    )?;
    if root.id().domain == *scope && supplied {
        Ok(())
    } else {
        Err(sql::invalid("content has no verified root or supplied-byte pin for this object and kind"))
    }
}

/// Expansion is bounded by one already validated manifest page. Existing pages
/// reached by new grants are queued for the same bounded expansion separately.
pub(super) fn expand(
    db: &Connection,
    object: &ObjectKey,
    kind: &str,
    root: &ManifestRoot,
    references: &[Reference],
    holding: &crate::cursor::HoldingPeriod,
) -> Result<()> {
    let valid:bool=db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_asset_access WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='manifest')",
        params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),kind,root.0.digest.as_str()],|row|row.get(0)
    )?;
    if !valid {
        return Ok(());
    }
    for reference in references {
        let child = match reference {
            Reference::Block { id, decoded_bytes } => {
                ContentRef::Single { block: id.clone(), decoded_bytes: *decoded_bytes }
            }
            Reference::Manifest { root } => ContentRef::Manifest { root: root.clone() },
        };
        grant_root(db, object, kind, &child, holding)?;
    }
    db.execute("UPDATE sync_asset_access SET expanded=1 WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='manifest'",params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),kind,root.0.digest.as_str()])?;
    Ok(())
}
