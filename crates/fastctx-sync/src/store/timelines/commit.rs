//! One installation commit: the head moves from the root this installer planned
//! against to the new one, the position index follows with exact preconditions,
//! and the watermarks readers resume from advance in the same transaction.
//!
//! A snapshot is derived local state, never object content: the head row is
//! what retains its root from collection, readers hold older roots through
//! Timeline uses, and no read grant is ever issued for it. The pass's staging
//! closes in the same commit, so no crash can leave its pins behind.
use super::state::{self, Head};
use crate::{
    blocks::projection_commit,
    cursor::{LocalSequence, Revision, StreamCursor},
    identity::{ObjectKey, OperationId},
    manifest::ManifestRoot,
    store::{access::Custody, local_head, sql},
    timeline::{TimelineItem, index::IndexChange, position},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, params},
};

/// The head an installation leaves behind.
pub(super) struct Next {
    pub projection: Revision,
    pub through: StreamCursor,
    pub root: Option<ManifestRoot>,
    /// None while a branch switch waits for its position index to be rebuilt.
    pub indexed: Option<Revision>,
    pub applied: LocalSequence,
}
pub(super) enum Installed {
    Committed,
    /// Another installation moved the head; plan again from the new one.
    Moved,
}

fn same(found: &Option<Head>, expected: &Option<Head>) -> bool {
    match (found, expected) {
        (None, None) => true,
        (Some(found), Some(expected)) => {
            found.projection == expected.projection && found.through == expected.through && found.root == expected.root
        }
        _ => false,
    }
}

pub(super) fn install(
    db: &Connection,
    custody: &Custody,
    operation: &OperationId,
    expected: &Option<Head>,
    next: &Next,
    changes: &[IndexChange],
) -> Result<Installed> {
    let conversation = &custody.object;
    if !same(&state::head(db, conversation)?, expected) {
        // Planned against a head that is gone: the staged pages are unreferenced.
        projection_commit::close_staging(db, custody, operation)?;
        return Ok(Installed::Moved);
    }
    // While a branch switch rebuilds the index, rows are rebuilt from the tree
    // instead of patched against the old branch.
    let mut indexed = next.indexed;
    if indexed == Some(next.projection) {
        for change in changes {
            if !index(db, conversation, change)? {
                // Partial cache patches stay hidden by the same head commit.
                // Rebuild from the authoritative root rather than failing it.
                indexed = None;
                break;
            }
        }
    }
    let local = local_head::advance(db)?;
    let scope = &conversation.scope;
    db.prepare_cached("INSERT INTO sync_timeline_heads(hub,scope,conversation,projection,through_cursor,root,indexed,applied,local) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(hub,scope,conversation) DO UPDATE SET projection=excluded.projection,through_cursor=excluded.through_cursor,root=excluded.root,indexed=excluded.indexed,applied=excluded.applied,local=excluded.local")?
        .execute(params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            conversation.object.as_str(),
            next.projection.0.to_be_bytes().as_slice(),
            serde_json::to_vec(&next.through).map_err(|error| sql::invalid(error.to_string()))?,
            next.root.as_ref().map(|root| root.0.digest.as_str().to_owned()),
            indexed.map(|revision| revision.0.to_be_bytes().to_vec()),
            next.applied.0.to_be_bytes().as_slice(),
            local.0.to_be_bytes().as_slice()
        ])?;
    if indexed != Some(next.projection) {
        crate::scheduler::projection_changed(db, "derived.timeline-index", conversation, local)?;
    }
    projection_commit::close_staging(db, custody, operation)?;
    Ok(Installed::Committed)
}

fn deactivate(db: &Connection, conversation: &ObjectKey, previous: &TimelineItem) -> Result<bool> {
    let scope = &conversation.scope;
    let changed = db
        .prepare_cached("UPDATE sync_timeline_items SET active=0 WHERE hub=?1 AND scope=?2 AND conversation=?3 AND item=?4 AND revision=?5 AND active=1")?
        .execute(params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            conversation.object.as_str(),
            previous.id.as_str(),
            previous.revision.0.to_be_bytes().as_slice()
        ])?;
    Ok(changed == 1)
}

fn index(db: &Connection, conversation: &ObjectKey, change: &IndexChange) -> Result<bool> {
    match change {
        IndexChange::Put { previous, item } => {
            if let Some(previous) = previous
                && !deactivate(db, conversation, previous)?
            {
                return Ok(false);
            }
            let scope = &conversation.scope;
            let position = position::encode(&item.position).map_err(|error| sql::invalid(error.to_string()))?;
            let changed = db.prepare_cached("INSERT INTO sync_timeline_items(hub,scope,conversation,item,revision,accepted,position,body,turn,parent,active) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1) ON CONFLICT(hub,scope,conversation,item,revision) DO NOTHING")?
                .execute(params![
                    scope.hub.to_string(),
                    scope.scope.as_str(),
                    conversation.object.as_str(),
                    item.id.as_str(),
                    item.revision.0.to_be_bytes().as_slice(),
                    item.position.accepted.0.to_be_bytes().as_slice(),
                    position,
                    serde_json::to_vec(item.as_ref()).map_err(|error| sql::invalid(error.to_string()))?,
                    item.turn.as_str(),
                    item.parent.as_ref().map(|parent| parent.as_str())
                ])?;
            Ok(changed == 1)
        }
        IndexChange::Retract { previous, .. } => deactivate(db, conversation, previous),
    }
}
