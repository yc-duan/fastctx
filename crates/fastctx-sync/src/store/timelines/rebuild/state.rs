//! The progress CAS and a bounded row batch commit under the same head fence.
use super::*;
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Progress {
    pub projection: Revision,
    pub root: ManifestRoot,
    pub retiring: bool,
    pub after: Option<Vec<u8>>,
}
pub(super) fn progress(db: &Connection, object: &ObjectKey) -> Result<Option<Progress>> {
    let row=db.prepare_cached("SELECT projection,root,phase,after_position FROM sync_timeline_rebuilds WHERE hub=?1 AND scope=?2 AND conversation=?3")?
        .query_row(params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str()],|row|Ok((sql::counter(row,0)?,row.get::<_,String>(1)?,row.get::<_,bool>(2)?,row.get::<_,Option<Vec<u8>>>(3)?))).optional()?;
    row.map(|(projection, digest, retiring, after)| {
        Ok(Progress {
            projection: Revision(projection),
            root: ManifestRoot(crate::manifest::BlobId {
                domain: object.scope.clone(),
                algorithm: crate::manifest::DigestAlgorithm::Sha256,
                digest: crate::identity::Digest::try_from(digest).map_err(sql::invalid)?,
            }),
            retiring,
            after,
        })
    })
    .transpose()
}
pub(super) fn matches(head: &Head, expected: &Head) -> bool {
    head.projection == expected.projection && head.root == expected.root
}
pub(super) fn current(progress: &Progress, head: &Head) -> bool {
    progress.projection == head.projection && Some(&progress.root) == head.root.as_ref()
}
pub(super) fn commit(
    db: &Connection,
    object: &ObjectKey,
    head: &Head,
    expected: &Option<Progress>,
    prepared: &Prepared,
) -> Result<Step> {
    let Some(found) = super::super::state::head(db, object)? else { return Ok(Step::Moved) };
    if !matches(&found, head) {
        return Ok(Step::Moved);
    }
    if found.indexed == Some(found.projection) {
        return Ok(Step::Complete);
    }
    if progress(db, object)? != *expected {
        return Ok(Step::Moved);
    }
    let root = head.root.as_ref().ok_or_else(|| sql::invalid("an index rebuild needs a snapshot root"))?;
    let active = expected.as_ref().filter(|progress| current(progress, head));
    let next = match prepared {
        Prepared::Copy { rows, done } => {
            if active.is_some_and(|progress| progress.retiring) {
                return Ok(Step::Moved);
            }
            for row in rows {
                db.prepare_cached("INSERT INTO sync_timeline_items(hub,scope,conversation,item,revision,accepted,position,body,turn,parent,active,index_generation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?11) ON CONFLICT(hub,scope,conversation,item,revision) DO UPDATE SET accepted=excluded.accepted,position=excluded.position,body=excluded.body,turn=excluded.turn,parent=excluded.parent,active=1,index_generation=excluded.index_generation")?
                    .execute(params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),row.id.as_str(),row.revision.0.to_be_bytes().as_slice(),row.accepted.0.to_be_bytes().as_slice(),&row.position,&row.body,row.turn.as_str(),row.parent.as_ref().map(|id|id.as_str()),head.projection.0.to_be_bytes().as_slice()])?;
            }
            Progress {
                projection: head.projection,
                root: root.clone(),
                retiring: *done,
                after: rows
                    .last()
                    .map(|row| row.position.clone())
                    .or_else(|| active.and_then(|progress| progress.after.clone())),
            }
        }
        Prepared::Retire => {
            if !active.is_some_and(|progress| progress.retiring) {
                return Ok(Step::Moved);
            }
            let mut rows = stale(db, object, head.projection, "<", ROWS)?;
            if rows.len() < ROWS as usize {
                rows.extend(stale(db, object, head.projection, ">", ROWS - rows.len() as u32)?);
            }
            for (id, revision) in &rows {
                db.prepare_cached("UPDATE sync_timeline_items SET active=0 WHERE hub=?1 AND scope=?2 AND conversation=?3 AND item=?4 AND revision=?5 AND active=1 AND index_generation<>?6")?
                    .execute(params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),id,revision,head.projection.0.to_be_bytes().as_slice()])?;
            }
            if rows.len() < ROWS as usize {
                let changed=db.prepare_cached("UPDATE sync_timeline_heads SET indexed=projection WHERE hub=?1 AND scope=?2 AND conversation=?3 AND projection=?4 AND root=?5")?
                    .execute(params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),head.projection.0.to_be_bytes().as_slice(),root.0.digest.as_str()])?;
                if changed != 1 {
                    return Err(fastctx_storage::Error::with_kind(
                        fastctx_storage::ErrorKind::Busy,
                        "the index completion lost its head fence",
                    ));
                }
                db.prepare_cached("DELETE FROM sync_timeline_rebuilds WHERE hub=?1 AND scope=?2 AND conversation=?3")?
                    .execute(params![
                        object.scope.hub.to_string(),
                        object.scope.scope.as_str(),
                        object.object.as_str()
                    ])?;
                return Ok(Step::Complete);
            }
            active.expect("retiring progress").clone()
        }
    };
    db.prepare_cached("INSERT INTO sync_timeline_rebuilds(hub,scope,conversation,projection,root,phase,after_position) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(hub,scope,conversation) DO UPDATE SET projection=excluded.projection,root=excluded.root,phase=excluded.phase,after_position=excluded.after_position")?
        .execute(params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),next.projection.0.to_be_bytes().as_slice(),next.root.0.digest.as_str(),next.retiring,&next.after])?;
    Ok(Step::More)
}
fn stale(
    db: &Connection,
    object: &ObjectKey,
    projection: Revision,
    comparison: &str,
    max: u32,
) -> Result<Vec<(String, Vec<u8>)>> {
    let query = match comparison {
        "<" => {
            "SELECT item,revision FROM sync_timeline_items INDEXED BY sync_timeline_generation WHERE hub=?1 AND scope=?2 AND conversation=?3 AND active=1 AND index_generation<?4 ORDER BY index_generation,item,revision LIMIT ?5"
        }
        _ => {
            "SELECT item,revision FROM sync_timeline_items INDEXED BY sync_timeline_generation WHERE hub=?1 AND scope=?2 AND conversation=?3 AND active=1 AND index_generation>?4 ORDER BY index_generation,item,revision LIMIT ?5"
        }
    };
    db.prepare_cached(query)?
        .query_map(
            params![
                object.scope.hub.to_string(),
                object.scope.scope.as_str(),
                object.object.as_str(),
                projection.0.to_be_bytes().as_slice(),
                max
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}
