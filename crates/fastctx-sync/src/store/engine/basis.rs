//! Local semantic inputs must come from an accepted version or a queued predecessor.

use crate::{
    catalog::ObjectHead,
    identity::DeviceId,
    manifest::ContentRef,
    store::{
        mutation::{BaseVersion, PendingState, ProposedObject},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) fn verify(
    db: &Connection,
    origin: &DeviceId,
    proposal: &ProposedObject,
    current: Option<&ObjectHead>,
) -> Result<()> {
    let key = &proposal.key;
    let base = match &proposal.base {
        BaseVersion::Absent => {
            let retained: bool = db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_object_versions WHERE hub=?1 AND scope=?2 AND object=?3) OR EXISTS(SELECT 1 FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3)")?.query_row(params![key.scope.hub.to_string(),key.scope.scope.as_str(),key.object.as_str()],|r|r.get(0))?;
            if current.is_some() || retained {
                return Err(sql::invalid("absent base names an existing object"));
            }
            None
        }
        BaseVersion::Version { content_version, root, .. } => {
            let saved = db.prepare_cached("SELECT content FROM sync_object_versions WHERE hub=?1 AND scope=?2 AND object=?3 AND authority_epoch=?4 AND revision=?5")?.query_row(params![key.scope.hub.to_string(),key.scope.scope.as_str(),key.object.as_str(),content_version.authority_epoch.bytes().as_slice(),content_version.revision.0.to_be_bytes().as_slice()],|r|r.get::<_,Option<Vec<u8>>>(0)).optional()?;
            let Some(saved) = saved else {
                return Err(sql::invalid("edit base is not a locally accepted object version"));
            };
            let saved: Option<ContentRef> = saved
                .map(|bytes| serde_json::from_slice(&bytes))
                .transpose()
                .map_err(|e| sql::invalid(e.to_string()))?;
            if &saved != root {
                return Err(sql::invalid("edit base root differs from its accepted version"));
            }
            saved
        }
    };
    let before = if let Some(predecessor) = &proposal.predecessor {
        let saved=db.prepare_cached("SELECT m.state,o.kind,o.candidate_root FROM sync_pending_objects o JOIN sync_pending_mutations m USING(hub,origin,operation) WHERE o.hub=?1 AND o.origin=?2 AND o.operation=?3 AND o.scope=?4 AND o.object=?5")?.query_row(params![key.scope.hub.to_string(),origin.as_str(),predecessor.as_str(),key.scope.scope.as_str(),key.object.as_str()],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<Vec<u8>>>(2)?))).optional()?;
        let Some((state, kind, candidate)) = saved else {
            return Err(sql::invalid("edit predecessor is not pending on this origin and object"));
        };
        let state: PendingState = serde_json::from_slice(&state).map_err(|e| sql::invalid(e.to_string()))?;
        if kind != proposal.kind
            || !matches!(state, PendingState::Queued | PendingState::Sending | PendingState::OutcomeUnknown)
        {
            return Err(sql::invalid("edit predecessor is no longer queued for this object kind"));
        }
        candidate
            .map(|bytes| serde_json::from_slice::<ContentRef>(&bytes))
            .transpose()
            .map_err(|e| sql::invalid(e.to_string()))?
    } else {
        base
    };
    if proposal.before != before {
        return Err(sql::invalid("edit before root is not its verified base or predecessor"));
    }
    Ok(())
}
