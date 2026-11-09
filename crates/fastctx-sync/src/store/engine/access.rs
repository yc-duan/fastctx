//! Admission is repeated inside the writer against current in-memory policy.
use crate::{
    cursor::{Holder, HoldingPeriod, Revision},
    identity::ScopeKey,
    registry::RegisteredKind,
    store::{
        access::{AccessContext, AccessPolicy, Custody, Decision, HoldingUse},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) fn current(db: &Connection, scope: &ScopeKey, revision: Revision) -> Result<Revision> {
    let state = db
        .query_row(
            "SELECT enabled,acl_revision,publication_epoch FROM sync_scopes WHERE hub=?1 AND scope=?2",
            params![scope.hub.to_string(), scope.scope.as_str()],
            |r| Ok((r.get::<_, bool>(0)?, sql::counter(r, 1)?, sql::counter(r, 2)?)),
        )
        .optional()?;
    match state {
        Some((true, acl, publication)) if acl == revision.0 => Ok(Revision(publication)),
        _ => Err(sql::invalid("scope is unavailable or its ACL fence changed")),
    }
}
pub(super) fn allowed(decision: Decision, revision: Revision) -> Result<()> {
    match decision {
        Decision::Allowed { revision: actual } if actual == revision => Ok(()),
        Decision::Denied { reason, .. } => Err(sql::invalid(reason)),
        _ => Err(sql::invalid("current permission snapshot is unavailable or stale")),
    }
}
pub(super) fn read(
    db: &Connection,
    policy: &dyn AccessPolicy,
    context: &AccessContext,
    kind: RegisteredKind,
) -> Result<()> {
    current(db, &context.scope, context.observed_acl)?;
    allowed(policy.can_read(context, kind), context.observed_acl)
}
pub(super) fn custody(
    db: &Connection,
    holder: &Holder,
    context: AccessContext,
    kind: RegisteredKind,
    object: crate::identity::ObjectKey,
    purpose: HoldingUse,
) -> Result<Custody> {
    let publication_epoch = current(db, &context.scope, context.observed_acl)?;
    if object.scope != context.scope {
        return Err(sql::invalid("object differs from the bound scope"));
    }
    Ok(Custody {
        holding: HoldingPeriod {
            holder: holder.clone(),
            scope: context.scope.clone(),
            acl_revision: context.observed_acl,
            publication_epoch,
        },
        context,
        kind,
        object,
        purpose,
    })
}
