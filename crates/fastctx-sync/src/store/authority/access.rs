//! Peer source eligibility and hub replica custody are different decisions.
use super::{SourceUpload, SqliteAuthority};
use crate::{
    cursor::{Holder, HoldingPeriod, Revision},
    identity::{ObjectKey, ProducerEpoch},
    registry::RegisteredKind,
    scope::{ScopeDescriptor, ScopeSubject},
    store::{
        Error,
        access::{AccessContext, Custody, Decision, HoldingUse, HostBinding, Principal},
        directory, sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) struct Bound {
    pub descriptor: ScopeDescriptor,
    pub source: Custody,
    pub hub: Custody,
}
pub(super) fn bind(
    authority: &SqliteAuthority,
    db: &Connection,
    context: &AccessContext,
    holding: &HoldingPeriod,
    kind: RegisteredKind,
    epoch: &ProducerEpoch,
) -> Result<Bound> {
    if context.scope.hub != *authority.run.hub() {
        return Err(sql::invalid("source belongs to another hub"));
    }
    let Principal::Account { account, device } = &context.principal else {
        return Err(sql::invalid("source requires the authenticated account and device"));
    };
    if holding.scope != context.scope
        || holding.acl_revision != context.observed_acl
        || holding.publication_epoch.0 == 0
        || holding.holder != (Holder::Device { device: device.clone() })
    {
        return Err(sql::invalid("source holding differs from its authenticated device or scope"));
    }
    let current = db
        .prepare_cached("SELECT enabled,acl_revision,publication_epoch FROM sync_scopes WHERE hub=?1 AND scope=?2")?
        .query_row(params![context.scope.hub.to_string(), context.scope.scope.as_str()], |r| {
            Ok((r.get::<_, bool>(0)?, sql::counter(r, 1)?, sql::counter(r, 2)?))
        })
        .optional()?;
    let Some((true, acl, publication)) = current else {
        return Err(sql::invalid("source scope is unavailable"));
    };
    if acl != context.observed_acl.0 {
        return Err(sql::invalid("source scope ACL moved"));
    }
    let descriptor =
        directory::described(db, &context.scope)?.ok_or_else(|| sql::invalid("source scope has no hub description"))?;
    if descriptor.acl_revision != context.observed_acl || descriptor.lifecycle != crate::catalog::Lifecycle::Live {
        return Err(sql::invalid("source description is stale or closed"));
    }
    let ScopeSubject::Conversation { conversation, .. } = &descriptor.subject else {
        return Err(fastctx_storage::Error::with_kind(
            fastctx_storage::ErrorKind::Unavailable,
            "this source needs its registered job or workspace provenance",
        ));
    };
    let object = ObjectKey { scope: context.scope.clone(), object: conversation.clone() };
    let source = Custody {
        context: context.clone(),
        kind,
        object: object.clone(),
        purpose: HoldingUse::RuntimeSource {
            host: HostBinding {
                conversation: object.clone(),
                account: account.clone(),
                host: device.clone(),
                epoch: epoch.clone(),
            },
        },
        holding: holding.clone(),
    };
    allowed(authority.blocks.policy().can_keep(context, kind, &object, &source.purpose), context.observed_acl)?;
    let hub_context =
        AccessContext { principal: Principal::LocalService, scope: context.scope.clone(), observed_acl: Revision(acl) };
    let hub = Custody {
        context: hub_context,
        kind,
        object,
        purpose: HoldingUse::Replica,
        holding: HoldingPeriod {
            holder: authority.blocks.holder().clone(),
            scope: context.scope.clone(),
            acl_revision: Revision(acl),
            publication_epoch: Revision(publication),
        },
    };
    allowed(authority.blocks.policy().can_keep(&hub.context, kind, &hub.object, &hub.purpose), Revision(acl))?;
    let deleted: bool = db
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3)")?
        .query_row(
            params![context.scope.hub.to_string(), context.scope.scope.as_str(), hub.object.object.as_str()],
            |r| r.get(0),
        )?;
    if deleted {
        return Err(sql::invalid("deleted source objects cannot accept new facts"));
    }
    Ok(Bound { descriptor, source, hub })
}
fn allowed(decision: Decision, revision: Revision) -> Result<()> {
    match decision {
        Decision::Allowed { revision: current } if current == revision => Ok(()),
        Decision::Unavailable => Err(fastctx_storage::Error::with_kind(
            fastctx_storage::ErrorKind::Unavailable,
            "source custody policy is unavailable",
        )),
        _ => Err(sql::invalid("current policy denies source custody")),
    }
}
pub(super) fn upload(authority: &SqliteAuthority, request: &SourceUpload) -> std::result::Result<Custody, Error> {
    let kind = crate::registry::REGISTRY.asset(&request.kind).map_err(Error::Invalid)?.registered();
    if !authority.authorizers.contains_key(kind.registration().id) {
        return Err(Error::Unavailable("no source authorizer is installed for uploads of this kind".into()));
    }
    authority
        .blocks
        .database()
        .read_now(|db| {
            bind(authority, db, &request.context, &request.holding, kind, &request.epoch).map(|bound| bound.hub)
        })
        .map_err(|e| sql::error(e, request.operation.clone()))
}
