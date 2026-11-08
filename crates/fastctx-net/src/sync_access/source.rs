use super::{Snapshot, SourcePhase};
use fastctx_sync::{
    cursor::Holder, identity::ObjectKey, registry::RegisteredKind, scope::ScopeSubject, store::access::HoldingUse,
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
    let ScopeSubject::Conversation { conversation, .. } = &descriptor.subject else { return false };
    let matches_conversation = |key: &ObjectKey| key.scope == descriptor.scope && key.object == *conversation;
    match purpose {
        HoldingUse::Replica => false,
        HoldingUse::RuntimeSource { host } => {
            kind.registration().assisted_host
                && host.host == *device
                && host.account == descriptor.owner.id
                && matches_conversation(&host.conversation)
        }
        HoldingUse::JobSource { job } => {
            kind.registration().job_source
                && job.executor == *device
                && job.account == descriptor.owner.id
                && job.job.scope == descriptor.scope
                && matches_conversation(&job.conversation)
        }
        HoldingUse::WorkspaceSource { workspace } => {
            kind.registration().workspace_source
                && workspace.machine == *device
                && workspace.account == descriptor.owner.id
                && matches_conversation(&workspace.conversation)
        }
    }
}
