//! A scope's change feed. The hub numbers each scope's changes without gaps and
//! keeps every change with its canonical bytes and the prefix digest through it,
//! so any holder can prove which history its cursor continues.
use crate::{
    catalog::{CatalogChange, Lifecycle, ObjectHead},
    cursor::{Revision, ScopeCursor, Sequence},
    identity::{Digest, FeedEpoch, OperationId, ScopeKey},
    store::sql,
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::sync::feed;

/// Opens `scope`'s feed under `epoch` at sequence zero; an open feed stays as it
/// is. Either way returns the feed's current cursor.
pub(crate) fn open(db: &Connection, scope: &ScopeKey, epoch: &FeedEpoch) -> Result<ScopeCursor> {
    let genesis = feed::genesis(scope, epoch);
    db.prepare_cached("INSERT INTO sync_scope_heads(hub,scope,epoch,sequence,snapshot,prefix_digest) VALUES(?1,?2,?3,?4,NULL,?5) ON CONFLICT(hub,scope) DO NOTHING")?
        .execute(params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            epoch.as_str(),
            0_u64.to_be_bytes().as_slice(),
            genesis.as_str()
        ])?;
    cursor(db, scope)?.ok_or_else(|| sql::invalid("the scope's feed has no scope row"))
}

/// The feed's current cursor, or None before the feed is opened.
pub(crate) fn cursor(db: &Connection, scope: &ScopeKey) -> Result<Option<ScopeCursor>> {
    db.prepare_cached("SELECT h.epoch,h.sequence,h.prefix_digest,s.acl_revision FROM sync_scope_heads h JOIN sync_scopes s USING(hub,scope) WHERE h.hub=?1 AND h.scope=?2")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, sql::counter(row, 1)?, row.get::<_, String>(2)?, sql::counter(row, 3)?))
        })
        .optional()?
        .map(|(epoch, through, prefix, acl)| {
            Ok(ScopeCursor {
                scope: scope.clone(),
                epoch: FeedEpoch::try_from(epoch).map_err(sql::invalid)?,
                acl_revision: Revision(acl),
                view: None,
                through: Sequence(through),
                prefix_digest: Some(Digest::try_from(prefix).map_err(sql::invalid)?),
            })
        })
        .transpose()
}

/// Records `head` as its object's current head and appends the change to its
/// scope's feed, in the caller's transaction. Returns the advanced cursor.
pub(crate) fn publish(db: &Connection, operation: &OperationId, head: &ObjectHead) -> Result<ScopeCursor> {
    let scope = &head.key.scope;
    let current = cursor(db, scope)?.ok_or_else(|| sql::invalid("the scope's feed has not been opened"))?;
    if head.acl_revision != current.acl_revision {
        return Err(sql::invalid("a published head names another ACL generation"));
    }
    let previous = current.prefix_digest.clone().ok_or_else(|| sql::invalid("an open feed has no prefix"))?;
    let sequence = Sequence(current.through.0.checked_add(1).ok_or_else(|| sql::invalid("the feed is exhausted"))?);
    let change = CatalogChange { sequence, operation: operation.clone(), head: head.clone() };
    let (canonical, prefix) = feed::advance(&previous, &change).map_err(sql::invalid)?;
    let (hub, name, object) = (scope.hub.to_string(), scope.scope.as_str(), head.key.object.as_str());
    let state = match head.lifecycle {
        Lifecycle::Live => "live",
        Lifecycle::Removed => "removed",
        Lifecycle::Deleted => "deleted",
    };
    db.prepare_cached("INSERT INTO sync_objects(hub,scope,object,kind,schema,authority,evidence,authority_epoch,revision,current_authority_epoch,current_version,content,history,state,completeness) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) ON CONFLICT(hub,scope,object) DO UPDATE SET kind=excluded.kind,schema=excluded.schema,authority=excluded.authority,evidence=excluded.evidence,authority_epoch=excluded.authority_epoch,revision=excluded.revision,current_authority_epoch=excluded.current_authority_epoch,current_version=excluded.current_version,content=excluded.content,history=excluded.history,state=excluded.state,completeness=excluded.completeness")?
        .execute(params![
            hub,
            name,
            object,
            head.kind,
            head.schema,
            json(&head.authority)?,
            json(&head.evidence)?,
            head.revision.authority_epoch.bytes().as_slice(),
            head.revision.revision.0.to_be_bytes().as_slice(),
            head.current_version.authority_epoch.bytes().as_slice(),
            head.current_version.revision.0.to_be_bytes().as_slice(),
            head.content.as_ref().map(json).transpose()?,
            head.history.as_ref().map(|root| root.0.digest.as_str().to_owned()),
            state,
            json(&head.completeness)?
        ])?;
    db.prepare_cached("INSERT INTO sync_changes(hub,scope,epoch,sequence,object,authority_epoch,revision,operation,canonical_change,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")?
        .execute(params![
            hub,
            name,
            current.epoch.as_str(),
            sequence.0.to_be_bytes().as_slice(),
            object,
            head.revision.authority_epoch.bytes().as_slice(),
            head.revision.revision.0.to_be_bytes().as_slice(),
            operation.as_str(),
            canonical,
            prefix.as_str()
        ])?;
    db.prepare_cached("UPDATE sync_scope_heads SET sequence=?3,prefix_digest=?4 WHERE hub=?1 AND scope=?2")?
        .execute(params![hub, name, sequence.0.to_be_bytes().as_slice(), prefix.as_str()])?;
    Ok(ScopeCursor { through: sequence, prefix_digest: Some(prefix), ..current })
}

/// A head's encoded column, as the engine's object reads decode it.
fn json(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| sql::invalid(error.to_string()))
}
