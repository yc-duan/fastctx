//! Incremental closure work is bounded by a parent page or one object/page grant.
use super::SqliteBlocks;
use crate::{
    blocks::{access, pages},
    cursor::HoldingPeriod,
    identity::{Digest, ObjectId, ObjectKey},
    scheduler::{self, Lease, Outcome},
    store::{
        Error,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) async fn process(store: &SqliteBlocks, lease: Lease) -> std::result::Result<Outcome, Error> {
    if lease.task.key.kind != "sync.manifest" {
        return Err(Error::Invalid("content worker received another handler's lease".into()));
    }
    let holding =
        lease.task.holding.clone().ok_or_else(|| Error::Invalid("content work needs a holding period".into()))?;
    if holding.holder != store.holder || holding.scope != lease.task.key.scope {
        return Err(Error::Denied("content work belongs to another holder or scope".into()));
    }
    let parts: Vec<_> = lease.task.key.key.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(Error::Invalid("invalid content task key".into()));
    }
    let command = parts[0].to_string();
    let hash = Digest::try_from(parts[1].to_string()).map_err(Error::Invalid)?;
    let through = lease.task.wanted;
    if command == "metadata" {
        let after = parts.get(2).copied();
        if let Some(cursor) = after {
            let (operation, object) =
                cursor.split_once('/').ok_or_else(|| Error::Invalid("invalid index custody cursor".into()))?;
            crate::identity::OperationId::try_from(operation.to_string()).map_err(Error::Invalid)?;
            ObjectId::try_from(object.to_string()).map_err(Error::Invalid)?;
        }
        return super::metadata_work::process(store, holding, hash, through, after).await;
    }
    let after = parts.get(2).map(|text| Digest::try_from(text.to_string())).transpose().map_err(Error::Invalid)?;
    store
        .db
        .write(1024 * 1024, move |db| {
            let _boundary = boundary::enter_typed_write(Role::Content);
            if !current(db, &holding)? {
                return Ok(Outcome::Superseded);
            }
            match command.as_str() {
                "complete" => {
                    completion(db, &holding, &hash, after.as_ref())?;
                    Ok(Outcome::Completed { through })
                }
                "access" if after.is_none() => expansion(db, &holding, &hash, through),
                _ => Err(sql::invalid("unknown content work operation")),
            }
        })
        .await
        .map_err(|error| sql::error(error, store.worker.clone()))
}
pub(super) fn current(db: &Connection, holding: &HoldingPeriod) -> Result<bool> {
    let state = db
        .query_row(
            "SELECT enabled,acl_revision,publication_epoch FROM sync_scopes WHERE hub=?1 AND scope=?2",
            params![holding.scope.hub.to_string(), holding.scope.scope.as_str()],
            |row| Ok((row.get::<_, bool>(0)?, sql::counter(row, 1)?, sql::counter(row, 2)?)),
        )
        .optional()?;
    Ok(state == Some((true, holding.acl_revision.0, holding.publication_epoch.0)))
}
fn completion(db: &Connection, holding: &HoldingPeriod, hash: &Digest, after: Option<&Digest>) -> Result<()> {
    pages::refresh(db, &holding.scope, hash)?;
    let mut query=db.prepare_cached("SELECT DISTINCT parent FROM sync_asset_edges WHERE hub=?1 AND scope=?2 AND child=?3 AND parent>?4 ORDER BY parent LIMIT 65")?;
    let parents = query
        .query_map(
            params![
                holding.scope.hub.to_string(),
                holding.scope.scope.as_str(),
                hash.as_str(),
                after.map_or("", Digest::as_str)
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    for parent in parents.iter().take(64) {
        let digest = Digest::try_from(parent.clone()).map_err(sql::invalid)?;
        let pending: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3 AND validated=0)",
            params![holding.scope.hub.to_string(), holding.scope.scope.as_str(), digest.as_str()],
            |r| r.get(0),
        )?;
        if pending {
            scheduler::content_changed(db, holding, format!("metadata:{}", digest.as_str()))?;
        }
        if pages::refresh(db, &holding.scope, &digest)? {
            scheduler::content_changed(db, holding, format!("complete:{}", digest.as_str()))?;
        }
    }
    if parents.len() > 64 {
        scheduler::content_changed(db, holding, format!("complete:{}:{}", hash.as_str(), parents[63]))?;
    }
    Ok(())
}
fn expansion(
    db: &Connection,
    holding: &HoldingPeriod,
    hash: &Digest,
    through: crate::cursor::LocalSequence,
) -> Result<Outcome> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3 AND validated=1)",
        params![holding.scope.hub.to_string(), holding.scope.scope.as_str(), hash.as_str()],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(Outcome::Blocked {
            dependency: format!("manifest:{}:{}:{}", holding.scope.hub, holding.scope.scope.as_str(), hash.as_str()),
            reason: "waiting for the referenced manifest page".into(),
        });
    }
    let next=db.query_row("SELECT object,kind FROM sync_asset_access WHERE hub=?1 AND scope=?2 AND hash=?3 AND expanded=0 AND root_kind='manifest' ORDER BY object,kind LIMIT 1",params![holding.scope.hub.to_string(),holding.scope.scope.as_str(),hash.as_str()],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()?;
    if let Some((object, kind)) = next {
        let object =
            ObjectKey { scope: holding.scope.clone(), object: ObjectId::try_from(object).map_err(sql::invalid)? };
        for child in pages::references(db, &holding.scope, hash)? {
            access::grant_root(db, &object, &kind, &child, holding)?;
        }
        db.execute("UPDATE sync_asset_access SET expanded=1 WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='manifest'",params![holding.scope.hub.to_string(),holding.scope.scope.as_str(),object.object.as_str(),kind,hash.as_str()])?;
        let remaining:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sync_asset_access WHERE hub=?1 AND scope=?2 AND hash=?3 AND expanded=0 AND root_kind='manifest')",params![holding.scope.hub.to_string(),holding.scope.scope.as_str(),hash.as_str()],|row|row.get(0))?;
        if remaining {
            return Ok(Outcome::Retry {
                next_due_millis: now(),
                reason: "remaining object grants for this manifest page".into(),
            });
        }
    }
    Ok(Outcome::Completed { through })
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}
