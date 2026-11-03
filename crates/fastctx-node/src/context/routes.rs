//! Inputs used to choose a plan or history copy. Keep metadata, never bodies.

use super::{dependencies::hash, parts::Part};
use crate::Node;
use fastctx_types::{
    MachineName, ResidentId, TownName, access::HubAccount, owned_records::Conversation, placement::Resource,
    residents::About,
};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Clone)]
pub(crate) struct Route {
    pub account: HubAccount,
    pub resource: Resource,
    pub ownership: Option<Conversation>,
    pub speaking: Option<MachineName>,
    pub selected: Option<MachineName>,
    pub legacy: bool,
}

#[derive(Default)]
pub(crate) struct Routes {
    copies: Mutex<BTreeMap<ResidentId, BTreeMap<u8, Route>>>,
    authors: Mutex<BTreeMap<ResidentId, Vec<MachineName>>>,
}

impl Routes {
    pub(crate) fn observe(&self, id: &ResidentId, route: Route) {
        let key = match route.resource {
            Resource::Plan => 0,
            Resource::History { scope: fastctx_types::history::Scope::Project { .. } } => 1,
            Resource::History { .. } => 2,
            _ => return,
        };
        self.copies.lock().expect("never poisoned").entry(id.clone()).or_default().insert(key, route);
    }

    pub(super) fn signature(
        &self,
        node: &Node,
        about: &About,
        town: Option<&TownName>,
        part: Part,
        cache: &mut BTreeMap<u64, u64>,
    ) -> Option<u64> {
        let routes: Vec<_> = self
            .copies
            .lock()
            .expect("never poisoned")
            .get(&about.id)?
            .iter()
            .filter(|(key, _)| match part {
                Part::Guide => **key == 0,
                Part::History => **key == 2 || (**key == 1 && town.is_some()),
                _ => false,
            })
            .map(|(_, route)| route.clone())
            .collect();
        if routes.is_empty() {
            return None;
        }
        let mut signatures = Vec::new();
        for route in routes {
            let ownership = route.ownership.as_ref().map(|owner| (&owner.account, &owner.shared_on));
            let resource = match &route.resource {
                Resource::History { scope: fastctx_types::history::Scope::Project { project } } => Some(project),
                _ => None,
            };
            let key = hash(&(
                &route.account,
                resource,
                ownership,
                &about.focus,
                &route.speaking,
                &route.selected,
                route.legacy,
            ));
            signatures.push(*cache.entry(key).or_insert_with(|| route.signature(node, about)));
        }
        Some(hash(&signatures))
    }

    pub(crate) fn remember_authors(&self, id: &ResidentId, authors: Vec<MachineName>) {
        self.authors.lock().expect("never poisoned").insert(id.clone(), authors);
    }

    pub(super) fn authors(&self, node: &Node, id: &ResidentId, acting: &fastctx_types::access::Acting) -> Option<u64> {
        let names = self.authors.lock().expect("never poisoned").get(id)?.clone();
        Some(hash(
            &names.iter().map(|name| (name, node.authorize_machine(acting, &name.into()).is_ok())).collect::<Vec<_>>(),
        ))
    }

    pub(super) fn retain(&self, live: &std::collections::BTreeSet<ResidentId>) {
        self.copies.lock().expect("never poisoned").retain(|id, _| live.contains(id));
        self.authors.lock().expect("never poisoned").retain(|id, _| live.contains(id));
    }
}

impl Route {
    fn signature(&self, node: &Node, about: &About) -> u64 {
        let Some(link) = node.link_of(&self.account.hub) else {
            return hash(&"missing hub");
        };
        let view = match crate::authorization::current_view(&link) {
            Ok(view) => view,
            Err(error) => return hash(&error.message),
        };
        let ranked = if self.legacy {
            let subject = fastctx_types::access::Acting::one(self.account.hub.clone(), self.account.account.clone());
            match crate::placement::destinations(node, &subject) {
                Ok(choices) => {
                    crate::placement::ranked(&choices, &about.focus, Some(&node.machine()), self.speaking.as_ref())
                        .into_iter()
                        .map(|choice| choice.machine.clone())
                        .collect()
                }
                Err(error) => return hash(&error.message),
            }
        } else {
            crate::replicas::location::candidates(
                node,
                &view,
                &self.account,
                &self.resource,
                self.ownership.as_ref(),
                &about.focus,
                self.speaking.as_ref(),
            )
        };
        self.selected_inputs(&view, ranked)
    }

    fn selected_inputs(
        &self,
        view: &fastctx_net::directory::View,
        mut ranked: Vec<fastctx_types::access::Written>,
    ) -> u64 {
        // Once an earlier candidate succeeded, later candidates were never
        // consulted. Their arrival cannot change this selection.
        if let Some(selected) = &self.selected
            && let Some(index) = ranked.iter().position(|candidate| view.same(&candidate.name, selected))
        {
            ranked.truncate(index + 1);
        }
        let candidates: Vec<_> = ranked
            .iter()
            .map(|candidate| (candidate, view.member(&candidate.name).map(|member| member.since)))
            .collect();
        let ownership = self.ownership.as_ref().map(|owner| {
            (
                view.account(&owner.account).map(|account| (account.locked, account.opened)),
                owner
                    .shared_on
                    .as_ref()
                    .and_then(|machine| view.member(machine))
                    .map(|member| (member.key, &member.home)),
            )
        });
        hash(&(
            &self.account,
            view.account(&self.account.account).map(|account| (account.locked, account.opened)),
            ownership,
            view.shared_records,
            view.plan_copies,
            candidates,
        ))
    }
}
