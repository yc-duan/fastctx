//! Scope membership, grant segments and physical replica exposure share one policy.
use super::Snapshot;
use crate::{
    authorization::{Action, Lookup},
    record_ownership,
};
use fastctx_sync::{
    cursor::Holder,
    identity::DeviceId,
    scope::{AccountIdentity, ScopeSubject, ScopeVisibility},
};
use fastctx_types::MachineName;

pub(super) fn readable(state: &Snapshot, account: &AccountIdentity, real_user: bool) -> bool {
    let descriptor = state.scope.descriptor();
    let directory = state.directory.indexed();
    if matches!(descriptor.subject, ScopeSubject::Administrators) {
        return real_user && directory.account(&account.name).is_some_and(|a| a.is_super());
    }
    let Some(owner) = descriptor.owner.account() else {
        return false;
    };
    match &descriptor.subject {
        ScopeSubject::Conversation { .. } => {
            record_ownership::allows(&directory, &account.name, std::slice::from_ref(&owner.name), born_on(state))
        }
        ScopeSubject::Project { town, .. } => {
            directory.find_project(town).is_some_and(|p| p.owner == owner.name)
                && directory.authorize(&account.name, Action::Project(town)).is_ok()
        }
        ScopeSubject::Account | ScopeSubject::Device { .. } => account.id == owner.id,
        ScopeSubject::Machine { device } => machine(state, account, device),
        ScopeSubject::Administrators => false,
    }
}
fn machine(state: &Snapshot, account: &AccountIdentity, device: &DeviceId) -> bool {
    let descriptor = state.scope.descriptor();
    let Some(owner) = descriptor.owner.account() else {
        return false;
    };
    let Some(target) = state.directory.member(device) else {
        return false;
    };
    if target.home.as_ref() != Some(&owner.name)
        || state.directory.indexed().authorize(&account.name, Action::Hold(&target.name)).is_err()
    {
        return false;
    }
    match descriptor.visibility {
        ScopeVisibility::Current => true,
        ScopeVisibility::SinceGrant { opened_acl } => state
            .directory
            .grants
            .get(&(device.clone(), account.id.clone()))
            .is_some_and(|granted| *granted <= opened_acl),
    }
}
pub(super) fn born_on(state: &Snapshot) -> Option<&MachineName> {
    let ScopeSubject::Conversation { shared_on: Some(name), born_seal: Some(seal), .. } =
        &state.scope.descriptor().subject
    else {
        return None;
    };
    state.directory.indexed().member(name).and_then(record_ownership::seal).filter(|now| now == seal).map(|_| name)
}
pub(super) fn replica(state: &Snapshot) -> bool {
    let Holder::Device { device } = &state.holder else {
        return true;
    };
    let Some(member) = state.directory.member(device) else {
        return false;
    };
    let descriptor = state.scope.descriptor();
    if member.kind == crate::directory::Kind::Phone {
        return member.logins.iter().any(|name| {
            state
                .directory
                .names
                .get(name)
                .and_then(|id| state.directory.account(id))
                .is_some_and(|account| readable(state, account, true))
        });
    }
    if member.kind != crate::directory::Kind::Computer || !member.is_machine() || member.managed {
        return false;
    }
    if matches!(descriptor.subject, ScopeSubject::Administrators | ScopeSubject::Machine { .. }) {
        let may_keep = |name: &fastctx_types::AccountName| {
            state
                .directory
                .names
                .get(name)
                .and_then(|id| state.directory.account(id))
                .is_some_and(|account| readable(state, account, true))
        };
        return member.home.as_ref().is_some_and(|home| member.logins.contains(home) && may_keep(home))
            && state
                .directory
                .indexed()
                .shares_by_machine(&member.name)
                .iter()
                .all(|&i| may_keep(&state.directory.directory.shares[i].account));
    }
    let Some(owner) = descriptor.owner.account() else {
        return false;
    };
    let owners = std::slice::from_ref(&owner.name);
    match &descriptor.subject {
        ScopeSubject::Project { town, .. } => {
            state.directory.indexed().find_project(town).is_some_and(|p| p.owner == owner.name)
                && state.directory.directory.member_keeps_project(&member.name, town)
        }
        ScopeSubject::Conversation { .. } => {
            record_ownership::keeps(&state.directory.directory, member, owners, born_on(state))
        }
        ScopeSubject::Account | ScopeSubject::Device { .. } => {
            record_ownership::keeps(&state.directory.directory, member, owners, None)
        }
        _ => false,
    }
}
