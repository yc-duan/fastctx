//! A source's private checkpoint state, read back by its own adapter. Only the
//! closure of the current checkpoint is readable, and never by another reader.
use super::{SqliteBlocks, pages, pins};
use crate::{
    cursor::Revision,
    identity::ScopeKey,
    manifest::{ContentBlock, ContentRef},
    store::{
        Error,
        ingest::{SourceTarget, StateRead, StateReply},
        sql,
    },
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension, params},
};

/// Bounds the closure walk that proves a page or block belongs to the state.
const STATE_NODES: u32 = 65_536;
#[path = "source_state/current.rs"]
mod current;

impl SqliteBlocks {
    /// One page or block of `target`'s checkpoint state. A block read rechecks,
    /// after its IO, that the checkpoint did not advance or discard its state.
    pub(crate) fn read_state(&self, target: &SourceTarget, request: StateRead) -> Result<StateReply, Error> {
        let custody = &target.custody;
        let scope = &custody.object.scope;
        let (checkpoint, node) = match &request {
            StateRead::Manifest { checkpoint, root } => (*checkpoint, ContentRef::Manifest { root: root.clone() }),
            StateRead::Block { checkpoint, id } => {
                (*checkpoint, ContentRef::Single { block: id.clone(), decoded_bytes: 0 })
            }
        };
        let _lease = self.leases.read(node.id())?;
        let found = self
            .db
            .read_now(|db| {
                let Some((revision, root)) = current::checked(db, self, target)? else {
                    return Ok(Err(None));
                };
                let Some(root) = root.filter(|_| revision == checkpoint) else {
                    return Ok(Err(Some(revision)));
                };
                let length = member(db, scope, &root, &node)?
                    .ok_or_else(|| sql::invalid("the requested content is not part of this checkpoint's state"))?;
                Ok(Ok((root, length)))
            })
            .map_err(|error| sql::error(error, self.worker.clone()))?;
        let (root, length) = match found {
            Ok(found) => found,
            Err(current) => return Ok(StateReply::Superseded { current }),
        };
        match request {
            StateRead::Manifest { root: page, .. } => self
                .db
                .read_now(|db| {
                    let found = current::checked(db, self, target)?;
                    match found {
                        Some((revision, Some(still))) if revision == checkpoint && still == root => {
                            pages::get(db, &page).map(StateReply::Manifest)
                        }
                        current => Ok(StateReply::Superseded { current: current.map(|(revision, _)| revision) }),
                    }
                })
                .map_err(|error| sql::error(error, self.worker.clone())),
            StateRead::Block { id, .. } => {
                let length = length.ok_or_else(|| Error::Invalid("the requested state block is a manifest".into()))?;
                let block = ContentBlock { id, decoded_bytes: length };
                let (bytes, _buffer) = super::metadata_work::payload_bytes(self, &block, 0)?
                    .ok_or_else(|| Error::Unavailable("the checkpoint's state block is not stored".into()))?;
                let current = self
                    .db
                    .read_now(|db| current::checked(db, self, target))
                    .map_err(|error| sql::error(error, self.worker.clone()))?;
                match current {
                    Some((revision, Some(still))) if revision == checkpoint && still == root => {
                        Ok(StateReply::Bytes(bytes))
                    }
                    current => Ok(StateReply::Superseded { current: current.map(|(revision, _)| revision) }),
                }
            }
        }
    }
}

/// The decoded length of `node` if it lies in `root`'s closure: Some(None) for
/// a manifest, Some(Some(bytes)) for a block, None when it is not a member.
fn member(db: &Connection, scope: &ScopeKey, root: &ContentRef, node: &ContentRef) -> SqlResult<Option<Option<u32>>> {
    let (form, length) = pins::shape(root);
    let wanted = pins::shape(node).0;
    db.prepare_cached("WITH RECURSIVE closure(hash,form,length) AS (SELECT ?3,?4,?5 UNION SELECT e.child,e.child_kind,e.decoded_bytes FROM sync_asset_edges e JOIN closure c ON e.parent=c.hash AND c.form='manifest' WHERE e.hub=?1 AND e.scope=?2 LIMIT ?8) SELECT length FROM closure WHERE hash=?6 AND form=?7 LIMIT 1")?
        .query_row(
            params![
                scope.hub.to_string(),
                scope.scope.as_str(),
                root.id().digest.as_str(),
                form,
                length,
                node.id().digest.as_str(),
                wanted,
                STATE_NODES
            ],
            |row| row.get::<_, Option<u32>>(0),
        )
        .optional()
        .map_err(Into::into)
}
