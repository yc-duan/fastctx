//! The ordered replicas eligible for a resource, without performing I/O.
use fastctx_net::directory::View;
use fastctx_types::{
    MachineName,
    access::{HubAccount, Written},
    focus::Focus,
    owned_records::Conversation,
    placement::Resource,
};

pub(crate) struct Selection<'a> {
    pub view: &'a View,
    pub account: &'a HubAccount,
    pub resource: &'a Resource,
    pub ownership: Option<&'a Conversation>,
    pub focus: &'a Focus,
    pub here: &'a MachineName,
    pub speaking: Option<&'a MachineName>,
}

impl Selection<'_> {
    pub(crate) fn candidates(self, allowed: impl Fn(&Written) -> bool) -> Vec<Written> {
        let Self { view, account, resource, ownership, focus, here, speaking } = self;
        let mut candidates = Vec::new();
        for member in view.machines().filter(|member| member.online) {
            let keeps = match &resource {
                Resource::History { scope: fastctx_types::history::Scope::Project { project } } => view
                    .project_named(&account.account, project)
                    .is_some_and(|project| view.member_keeps_project(&member.name, &project.town)),
                _ => ownership.is_some_and(|conversation| {
                    fastctx_net::record_ownership::keeps(
                        &view.directory,
                        member,
                        std::slice::from_ref(&conversation.account),
                        conversation.shared_on.as_ref(),
                    )
                }),
            };
            let machine = Written { name: member.name.clone(), hub: Some(view.hub_name.clone()) };
            if keeps && allowed(&machine) {
                candidates.push(machine);
            }
        }
        let mut ranked = Vec::new();
        let mut add = |machine: &fastctx_types::MachineName| {
            if let Some(candidate) = candidates.iter().find(|candidate| view.same(&candidate.name, machine))
                && !ranked.contains(candidate)
            {
                ranked.push(candidate.clone());
            }
        };
        add(here);
        for focused in &focus.machines {
            add(&focused.machine.name);
        }
        if let Some(speaking) = speaking {
            add(speaking);
        }
        if candidates.len() == 1 {
            add(&candidates[0].name);
        }
        ranked
    }
}
