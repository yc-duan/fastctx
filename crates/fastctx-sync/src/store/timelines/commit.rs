//! One installation commit: the head moves from the root this installer planned
//! against to the new one, the position index follows with exact preconditions,
//! and the watermarks readers resume from advance in the same transaction.
use super::evidence::{self, Head};
use crate::{
    blocks::retain_supplied,
    cursor::{LocalSequence, Revision, StreamCursor},
    identity::ObjectKey,
    manifest::{ContentRef, ManifestRoot},
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
    expected: &Option<Head>,
    next: &Next,
    changes: &[IndexChange],
) -> Result<Installed> {
    let conversation = &custody.object;
    if !same(&evidence::head(db, conversation)?, expected) {
        return Ok(Installed::Moved);
    }
    if let Some(root) = &next.root
        && next.root != expected.as_ref().and_then(|head| head.root.clone())
    {
        // The installer's own ready pins still hold the new pages, so no
        // collection can claim them before this grant and the head retain them.
        retain_supplied(db, custody, &ContentRef::Manifest { root: root.clone() })?;
    }
    // While a branch switch rebuilds the index, rows are rebuilt from the tree
    // instead of patched against the old branch.
    let indexing = next.indexed == Some(next.projection);
    if indexing {
        for change in changes {
            index(db, conversation, change)?;
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
            next.indexed.map(|revision| revision.0.to_be_bytes().to_vec()),
            next.applied.0.to_be_bytes().as_slice(),
            local.0.to_be_bytes().as_slice()
        ])?;
    Ok(Installed::Committed)
}

fn deactivate(db: &Connection, conversation: &ObjectKey, previous: &TimelineItem) -> Result<()> {
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
    if changed != 1 {
        return Err(sql::invalid("the position index differs from the snapshot it was planned against"));
    }
    Ok(())
}

fn index(db: &Connection, conversation: &ObjectKey, change: &IndexChange) -> Result<()> {
    match change {
        IndexChange::Put { previous, item } => {
            if let Some(previous) = previous {
                deactivate(db, conversation, previous)?;
            }
            let scope = &conversation.scope;
            let position = position::encode(&item.position).map_err(|error| sql::invalid(error.to_string()))?;
            db.prepare_cached("INSERT INTO sync_timeline_items(hub,scope,conversation,item,revision,accepted,position,body,turn,parent,active) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1)")?
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
        }
        IndexChange::Retract { previous, .. } => deactivate(db, conversation, previous)?,
    }
    Ok(())
}
