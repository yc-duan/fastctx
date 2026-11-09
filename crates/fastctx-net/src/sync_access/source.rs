use super::{Snapshot, SourcePhase};
use fastctx_sync::{
    cursor::Holder,
    identity::ObjectKey,
    registry::RegisteredKind,
    scope::ScopeSubject,
    store::access::{HoldingUse, JobBinding, JobSourceOwner},
};

pub(super) fn keeps(state: &Snapshot, kind: RegisteredKind, object: &ObjectKey, purpose: &HoldingUse) -> bool {
    let Holder::Device { device } = &state.holder else { return false };
    if state
        .directory
        .member(device)
        .is_none_or(|member| !member.is_machine() || member.kind != crate::directory::Kind::Computer)
    {
        return false;
    }
    let Some(source) = &state.source else { return false };
    if source.object != *object
        || source.kind != kind.registration().id
        || source.binding != *purpose
        || source.phase == SourcePhase::Released
    {
        return false;
    }
    let descriptor = state.scope.descriptor();
    let Some(owner) = descriptor.owner.account() else { return false };
    let matches_conversation = |key: &ObjectKey| {
        matches!(&descriptor.subject, ScopeSubject::Conversation { conversation, .. }
            if key.scope == descriptor.scope && key.object == *conversation)
    };
    match purpose {
        HoldingUse::Replica => false,
        HoldingUse::RuntimeSource { host } => {
            kind.registration().assisted_host
                && host.host == *device
                && host.account == owner.id
                && matches_conversation(&host.conversation)
        }
        HoldingUse::JobSource { job } => {
            kind.registration().job_source
                && job.executor == *device
                && job.job.scope == descriptor.scope
                && state.directory.identity(&job.account).is_some()
                && job_owner(state, job)
        }
        HoldingUse::WorkspaceSource { workspace } => {
            kind.registration().workspace_source
                && workspace.machine == *device
                && workspace.account == owner.id
                && matches_conversation(&workspace.conversation)
        }
    }
}

fn job_owner(state: &Snapshot, job: &JobBinding) -> bool {
    let descriptor = state.scope.descriptor();
    let Some(owner) = descriptor.owner.account() else { return false };
    match &job.owner {
        JobSourceOwner::Resident { conversation } => {
            conversation.scope.hub == descriptor.scope.hub
                && job.account == owner.id
                && match &descriptor.subject {
                    ScopeSubject::Conversation { conversation: id, .. } => {
                        conversation.scope == descriptor.scope && conversation.object == *id
                    }
                    ScopeSubject::Account => true,
                    _ => false,
                }
        }
        JobSourceOwner::Page { project, page } => {
            if project.scope.hub != descriptor.scope.hub || page.scope != project.scope {
                return false;
            }
            // A project's owner can differ from the consenting job account.
            // SourceCustody already records the authenticated audience routing;
            // a caller-supplied PageSource cannot grant project-wide retention.
            match &descriptor.subject {
                ScopeSubject::Account => job.account == owner.id,
                ScopeSubject::Project { project: id, .. } => project.scope == descriptor.scope && project.object == *id,
                _ => false,
            }
        }
    }
}
