//! One row per scope records the current target and a bounded continuation.
use super::*;
use crate::identity::{ObjectId, OperationId, RootLeaseId, SourceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct UploadKey {
    pub operation: OperationId,
    pub object: ObjectId,
}

#[derive(Serialize, Deserialize)]
pub(super) enum Progress {
    Supply { after: Option<crate::blocks::supply::SupplyTicket> },
    Uses { after: Option<RootLeaseId> },
    Steps { after: Option<SourceId> },
    Uploads { after: Option<UploadKey> },
    Pins { after: Option<crate::blocks::uploads::PinCursor> },
    Done,
}
pub(super) struct State {
    pub generation: Revision,
    pub target: ScopeState,
    pub progress: Progress,
}
impl State {
    pub fn holding(&self, scope: &ScopeKey, holder: &Holder) -> HoldingPeriod {
        HoldingPeriod {
            scope: scope.clone(),
            holder: holder.clone(),
            acl_revision: self.target.acl_revision,
            publication_epoch: self.target.publication_epoch,
        }
    }
}
pub(super) fn encode(value: &impl Serialize) -> SqlResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| sql::invalid(error.to_string()))
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> SqlResult<T> {
    serde_json::from_slice(bytes).map_err(|error| sql::invalid(error.to_string()))
}
pub(super) fn get(db: &Connection, scope: &ScopeKey) -> SqlResult<Option<State>> {
    let row=db.query_row("SELECT generation,acl_revision,publication_epoch,enabled,progress FROM sync_scope_reconciliations WHERE hub=?1 AND scope=?2",params![scope.hub.to_string(),scope.scope.as_str()],|row|Ok((sql::counter(row,0)?,sql::counter(row,1)?,sql::counter(row,2)?,row.get::<_,bool>(3)?,row.get::<_,Vec<u8>>(4)?))).optional()?;
    row.map(|(generation, acl, epoch, enabled, progress)| {
        Ok(State {
            generation: Revision(generation),
            target: ScopeState { acl_revision: Revision(acl), publication_epoch: Revision(epoch), enabled },
            progress: decode(&progress)?,
        })
    })
    .transpose()
}
pub(crate) fn begin(db: &Connection, scope: &ScopeKey, generation: Revision, target: ScopeState) -> SqlResult<()> {
    db.execute("INSERT INTO sync_scope_reconciliations(hub,scope,generation,acl_revision,publication_epoch,enabled,progress) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(hub,scope) DO UPDATE SET generation=excluded.generation,acl_revision=excluded.acl_revision,publication_epoch=excluded.publication_epoch,enabled=excluded.enabled,progress=excluded.progress",
        params![scope.hub.to_string(),scope.scope.as_str(),generation.0.to_be_bytes().as_slice(),target.acl_revision.0.to_be_bytes().as_slice(),target.publication_epoch.0.to_be_bytes().as_slice(),target.enabled,encode(&Progress::Supply{after:None})?])?;
    Ok(())
}
pub(super) fn save(db: &Connection, scope: &ScopeKey, state: &State) -> SqlResult<()> {
    let changed = db.execute(
        "UPDATE sync_scope_reconciliations SET progress=?4 WHERE hub=?1 AND scope=?2 AND generation=?3",
        params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            state.generation.0.to_be_bytes().as_slice(),
            encode(&state.progress)?
        ],
    )?;
    if changed != 1 {
        return Err(sql::invalid("scope reconciliation lost its target generation"));
    }
    Ok(())
}
