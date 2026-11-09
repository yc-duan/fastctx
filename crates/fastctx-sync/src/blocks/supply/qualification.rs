//! Local supply is exact-operation evidence, separate from ordinary body access.
use super::{
    model::{Request, SupplyOwner},
    rows::Check,
};
use crate::{
    blocks::{pins, uploads},
    cursor::Revision,
    identity::{ScopeId, ScopeKey, SourceId},
    manifest::ContentRef,
    store::{roots::RootUse, source_step::StepKey, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) enum NodeSupply {
    Absent,
    Pending,
    Ready,
}

pub(super) fn open(db: &Connection, request: &Request) -> Result<Revision> {
    let custody = request.custody()?;
    let scope = &request.object.scope;
    let step = db
        .query_row(
            "SELECT scope,source,step FROM sync_source_steps WHERE hub=?1 AND operation=?2",
            params![scope.hub.to_string(), request.operation.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, sql::counter(row, 2)?)),
        )
        .optional()?
        .map(|(scope_name, source, revision)| {
            Ok::<_, fastctx_storage::Error>(StepKey {
                scope: ScopeKey { hub: scope.hub.clone(), scope: ScopeId::try_from(scope_name).map_err(sql::invalid)? },
                source: SourceId::try_from(source).map_err(sql::invalid)?,
                revision: Revision(revision),
            })
        })
        .transpose()?;
    match (&request.owner, &step) {
        (SupplyOwner::Mutation, Some(_)) => {
            return Err(sql::invalid("an edit cannot borrow a source step's private supply"));
        }
        (SupplyOwner::Source { .. }, None) => return Err(sql::invalid("source supply has no engine-issued step")),
        _ => {}
    }
    let owner = step.as_ref().map_or(uploads::Owner::Replica, uploads::Owner::Source);
    uploads::open(db, &custody, &request.operation, owner)?;
    let active = uploads::active_supply(db, &custody, &request.operation)?
        .ok_or_else(|| sql::invalid("supply operation is not active"))?;
    validate_owner(db, request, &active)?;
    Ok(active.revision)
}

pub(super) fn current(db: &Connection, check: &Check) -> Result<Option<Revision>> {
    let active = uploads::active_supply(db, &check.request.custody()?, &check.request.operation)?;
    if let Some(active) = active {
        validate_owner(db, &check.request, &active)?;
        Ok(Some(active.revision))
    } else {
        Ok(None)
    }
}
fn validate_owner(db: &Connection, request: &Request, active: &uploads::ActiveSupply) -> Result<()> {
    let source = match &request.owner {
        SupplyOwner::Source { source } => Some(source),
        SupplyOwner::Use { usage: RootUse::Checkpoint { source } } if active.source.is_some() => Some(source),
        SupplyOwner::Mutation if active.source.is_some() => {
            return Err(sql::invalid("mutation supply belongs to a source step"));
        }
        _ => None,
    };
    if let Some(source) = source {
        let step = active.source.as_ref().ok_or_else(|| sql::invalid("source supply lost its step owner"))?;
        let bound:bool=db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_source_step_targets WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4 AND target_scope=?5 AND target_source=?6 AND object=?7 AND kind=?8 AND active=1)",
            params![step.scope.hub.to_string(),step.scope.scope.as_str(),step.source.as_str(),step.revision.0.to_be_bytes().as_slice(),request.object.scope.scope.as_str(),source.as_str(),request.object.object.as_str(),request.kind],|row|row.get(0),
        )?;
        if !bound {
            return Err(sql::invalid("supply basis belongs to another bound source"));
        }
    }
    Ok(())
}

pub(super) fn supplied(db: &Connection, check: &Check, root: &ContentRef) -> Result<NodeSupply> {
    let request = &check.request;
    if root.id().domain != request.object.scope {
        return Err(sql::invalid("supplied node crosses its object scope"));
    }
    let scope = &request.object.scope;
    let (form, length) = pins::shape(root);
    let phase=db.query_row(
        "SELECT phase FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4 AND kind=?5 AND hash=?6 AND root_kind=?7 AND decoded_bytes IS ?8 AND holding=?9",
        params![scope.hub.to_string(),scope.scope.as_str(),request.operation.as_str(),request.object.object.as_str(),request.kind,root.id().digest.as_str(),form,length,super::model::encode(&request.holding)?],|row|row.get::<_,String>(0),
    ).optional()?;
    match phase.as_deref() {
        Some("staging") => return Ok(NodeSupply::Pending),
        Some("ready") => {}
        _ => return Ok(NodeSupply::Absent),
    }
    if let ContentRef::Manifest { root } = root {
        let validated: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3 AND validated=1)",
            params![scope.hub.to_string(), scope.scope.as_str(), root.0.digest.as_str()],
            |row| row.get(0),
        )?;
        if !validated {
            return Ok(NodeSupply::Pending);
        }
    }
    Ok(NodeSupply::Ready)
}
