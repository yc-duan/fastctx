use super::{DirectoryPolicy, Snapshot};
use crate::{
    authorization::{Action, Lookup},
    record_ownership,
};
use fastctx_sync::{
    catalog::Lifecycle,
    cursor::{Holder, Revision},
    identity::{DeviceId, ObjectKey},
    registry::{ConflictRule, DataClass, RegisteredKind, ScopePolicy},
    scope::{AccountIdentity, ScopeSubject},
    store::access::{AccessContext, AccessPolicy, Decision, HoldingUse, Principal},
};
use fastctx_types::{MachineName, access::Purpose};

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
        let applicable = kind.is_none_or(|kind| scope_matches(kind, &descriptor.subject));
        if descriptor.scope != context.scope
            || snapshot.directory.hub != context.scope.hub
            || !snapshot.enabled
            || descriptor.lifecycle != Lifecycle::Live
            || context.observed_acl != revision
            || object.is_some_and(|key| key.scope != context.scope)
            || !applicable
            || snapshot.directory.identity(&descriptor.owner.id) != Some(&descriptor.owner)
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
            actor(state, context)
                .is_some_and(|(account, device)| kind_reader(state, account, device, kind) && readable(state, account))
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
            if !kind_reader(state, account, device, kind) || !readable(state, account) {
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

fn scope_matches(kind: RegisteredKind, subject: &ScopeSubject) -> bool {
    matches!(
        (kind.registration().scope, subject),
        (ScopePolicy::Account, ScopeSubject::Account)
            | (ScopePolicy::Conversation, ScopeSubject::Conversation { .. })
            | (ScopePolicy::Project, ScopeSubject::Project { .. })
            | (ScopePolicy::Machine, ScopeSubject::Machine { .. })
            | (ScopePolicy::Device, ScopeSubject::Device { .. })
    )
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
fn kind_reader(state: &Snapshot, account: &AccountIdentity, device: &DeviceId, kind: RegisteredKind) -> bool {
    match kind.registration().id {
        "account.collaboration" | "account.directory" | "account.read-position" => {
            state.directory.member(device).is_some_and(|member| member.logins.contains(&account.name))
        }
        _ => true,
    }
}
fn readable(state: &Snapshot, account: &AccountIdentity) -> bool {
    let owner = &state.scope.descriptor().owner;
    let directory = state.directory.indexed();
    match &state.scope.descriptor().subject {
        ScopeSubject::Conversation { .. } => {
            record_ownership::allows(&directory, &account.name, std::slice::from_ref(&owner.name), born_on(state))
        }
        ScopeSubject::Project { town, .. } => {
            directory.find_project(town).is_some_and(|project| project.owner == owner.name)
                && directory.authorize(&account.name, Action::Project(town)).is_ok()
        }
        ScopeSubject::Account => account.id == owner.id,
        ScopeSubject::Machine { device } => state
            .directory
            .member(device)
            .is_some_and(|member| directory.authorize(&account.name, Action::Hold(&member.name)).is_ok()),
        ScopeSubject::Device { .. } => account.id == owner.id,
    }
}
fn born_on(state: &Snapshot) -> Option<&MachineName> {
    let ScopeSubject::Conversation { shared_on: Some(name), born_seal: Some(seal), .. } =
        &state.scope.descriptor().subject
    else {
        return None;
    };
    state.directory.indexed().member(name).and_then(record_ownership::seal).filter(|now| now == seal).map(|_| name)
}
fn replica(state: &Snapshot) -> bool {
    let Holder::Device { device } = &state.holder else { return true };
    let Some(member) = state.directory.member(device) else { return false };
    let descriptor = state.scope.descriptor();
    let owner = std::slice::from_ref(&descriptor.owner.name);
    if member.kind == crate::directory::Kind::Phone {
        return member.logins.iter().any(|name| {
            let Some(account) = state.directory.names.get(name).and_then(|id| state.directory.account(id)) else {
                return false;
            };
            // Phone filtering uses the same object readers, without execution custody.
            readable(state, account)
        });
    }
    if member.kind != crate::directory::Kind::Computer || !member.is_machine() {
        return false;
    }
    match &descriptor.subject {
        ScopeSubject::Project { town, .. } => {
            state.directory.indexed().find_project(town).is_some_and(|project| project.owner == descriptor.owner.name)
                && state.directory.directory.member_keeps_project(&member.name, town)
        }
        ScopeSubject::Conversation { .. } => {
            record_ownership::keeps(&state.directory.directory, member, owner, born_on(state))
        }
        ScopeSubject::Account | ScopeSubject::Device { .. } => {
            record_ownership::keeps(&state.directory.directory, member, owner, None)
        }
        ScopeSubject::Machine { device } => state.directory.member(device).is_some_and(|target| {
            member.home.as_ref().is_some_and(|home| {
                member.logins.contains(home)
                    && state.directory.indexed().authorize(home, Action::Hold(&target.name)).is_ok()
            }) && !member.managed
                && state.directory.indexed().shares_by_machine(&member.name).iter().all(|&index| {
                    state
                        .directory
                        .indexed()
                        .authorize(&state.directory.directory.shares[index].account, Action::Hold(&target.name))
                        .is_ok()
                })
        }),
    }
}
