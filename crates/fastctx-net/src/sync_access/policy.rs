use super::{
    DirectoryPolicy, Snapshot,
    readers::{readable, replica},
};
use crate::authorization::Action;
use fastctx_sync::{
    catalog::Lifecycle,
    cursor::Revision,
    identity::{DeviceId, ObjectKey},
    registry::{ConflictRule, DataClass, MachineVisibilityRule, ReaderClass, RegisteredKind, ScopePolicy},
    scope::{AccountIdentity, ScopeDescriptor, ScopeOwner, ScopeSubject, ScopeVisibility},
    store::access::{AccessContext, AccessPolicy, Decision, HoldingUse, Principal},
};
use fastctx_types::access::Purpose;

impl DirectoryPolicy {
    fn decide(
        &self,
        context: &AccessContext,
        kind: Option<RegisteredKind>,
        object: Option<&ObjectKey>,
        check: impl FnOnce(&Snapshot) -> bool,
    ) -> Decision {
        let Some(snapshot) = self.state.current(&context.scope, object) else { return Decision::Unavailable };
        let descriptor = snapshot.scope.descriptor();
        let revision = descriptor.acl_revision;
        let applicable = kind.is_none_or(|kind| scope_matches(kind, descriptor));
        if descriptor.scope != context.scope
            || snapshot.directory.hub != context.scope.hub
            || !snapshot.enabled
            || descriptor.lifecycle != Lifecycle::Live
            || context.observed_acl != revision
            || object.is_some_and(|key| key.scope != context.scope)
            || !applicable
            || match &descriptor.owner {
                ScopeOwner::Account { identity } => snapshot.directory.identity(&identity.id) != Some(identity),
                ScopeOwner::Hub => !matches!(descriptor.subject, ScopeSubject::Administrators),
            }
            || !check(&snapshot)
        {
            return denied(revision);
        }
        Decision::Allowed { revision }
    }
}
impl AccessPolicy for DirectoryPolicy {
    fn can_read(&self, context: &AccessContext, kind: RegisteredKind) -> Decision {
        self.decide(context, Some(kind), None, |state| {
            if matches!(context.principal, Principal::LocalService) {
                return reader_class(context, kind);
            }
            actor(state, context).is_some_and(|(account, device)| {
                kind_reader(state, context, account, device, kind)
                    && readable(state, account, matches!(context.principal, Principal::Account { .. }))
            })
        })
    }
    fn can_read_object(&self, context: &AccessContext, kind: RegisteredKind, object: &ObjectKey) -> Decision {
        self.decide(context, Some(kind), Some(object), |state| {
            if matches!(context.principal, Principal::LocalService) {
                return reader_class(context, kind)
                    && (replica(state)
                        || state
                            .source
                            .as_ref()
                            .is_some_and(|source| super::source::keeps(state, kind, object, &source.binding)));
            }
            actor(state, context).is_some_and(|(account, device)| {
                kind_reader(state, context, account, device, kind)
                    && readable(state, account, matches!(context.principal, Principal::Account { .. }))
            })
        })
    }
    fn can_write(&self, context: &AccessContext, kind: RegisteredKind) -> Decision {
        self.decide(context, Some(kind), None, |state| {
            if matches!(
                kind.registration().conflict,
                ConflictRule::SingleWriter | ConflictRule::LocalPending | ConflictRule::MigrationSource
            ) {
                return false;
            }
            let Some((account, device)) = actor(state, context) else { return false };
            if !kind_reader(state, context, account, device, kind)
                || !readable(state, account, matches!(context.principal, Principal::Account { .. }))
            {
                return false;
            }
            match &state.scope.descriptor().subject {
                ScopeSubject::Machine { device: target } => state.directory.member(target).is_some_and(|member| {
                    state.directory.indexed().authorize(&account.name, Action::Hold(&member.name)).is_ok()
                }),
                ScopeSubject::Device { device: target } => {
                    target == device && matches!(context.principal, Principal::Account { .. })
                }
                _ => true,
            }
        })
    }
    fn can_keep(
        &self,
        context: &AccessContext,
        kind: RegisteredKind,
        object: &ObjectKey,
        purpose: &HoldingUse,
    ) -> Decision {
        self.decide(context, Some(kind), Some(object), |state| {
            if kind.registration().class != DataClass::Asset {
                return false;
            }
            match purpose {
                HoldingUse::Replica => replica(state),
                _ => super::source::keeps(state, kind, object, purpose),
            }
        })
    }
    fn can_execute(&self, context: &AccessContext, target: &DeviceId) -> Decision {
        self.decide(context, None, None, |state| {
            let Some((account, source)) = actor(state, context) else { return false };
            let Some(from) = state.directory.member(source) else { return false };
            let Some(to) = state.directory.member(target) else { return false };
            state
                .directory
                .indexed()
                .authorize(&account.name, Action::Call { from: &from.name, to: &to.name, purpose: Purpose::Hands })
                .is_ok()
        })
    }
}
fn denied(revision: Revision) -> Decision {
    Decision::Denied {
        revision,
        reason: "current membership, scope or source custody does not permit this operation".into(),
    }
}

fn scope_matches(kind: RegisteredKind, descriptor: &ScopeDescriptor) -> bool {
    let class = match descriptor.subject {
        ScopeSubject::Account => ScopePolicy::Account,
        ScopeSubject::Conversation { .. } => ScopePolicy::Conversation,
        ScopeSubject::Project { .. } => ScopePolicy::Project,
        ScopeSubject::Machine { .. } => ScopePolicy::Machine,
        ScopeSubject::Device { .. } => ScopePolicy::Device,
        ScopeSubject::Administrators => ScopePolicy::Administrators,
    };
    kind.registration().scopes.contains(&class)
        && (class != ScopePolicy::Machine
            || kind.registration().machine_visibility != MachineVisibilityRule::SinceGrant
            || matches!(descriptor.visibility, ScopeVisibility::SinceGrant { .. }))
}
fn actor<'a>(state: &'a Snapshot, context: &'a AccessContext) -> Option<(&'a AccountIdentity, &'a DeviceId)> {
    let (id, device, agent) = match &context.principal {
        Principal::Account { account, device } => (account, device, false),
        Principal::Agent { account, device, .. } => (account, device, true),
        Principal::LocalService => return None,
    };
    let account = state.directory.account(id)?;
    let member = state.directory.member(device)?;
    let allowed = if agent {
        state.directory.indexed().authorize(&account.name, Action::ActFor(&member.name)).is_ok()
    } else {
        member.logins.contains(&account.name)
    };
    allowed.then_some((account, device))
}
fn reader_class(context: &AccessContext, kind: RegisteredKind) -> bool {
    let class = match context.principal {
        Principal::Account { .. } => ReaderClass::User,
        Principal::Agent { .. } => ReaderClass::Agent,
        Principal::LocalService => ReaderClass::Service,
    };
    kind.registration().readers.is_none_or(|allowed| allowed.contains(&class))
}
fn kind_reader(
    state: &Snapshot,
    context: &AccessContext,
    account: &AccountIdentity,
    device: &DeviceId,
    kind: RegisteredKind,
) -> bool {
    if !reader_class(context, kind) {
        return false;
    }

    match kind.registration().id {
        "account.collaboration" | "account.directory" | "account.read-position" => {
            state.directory.member(device).is_some_and(|member| member.logins.contains(&account.name))
        }
        _ => true,
    }
}
