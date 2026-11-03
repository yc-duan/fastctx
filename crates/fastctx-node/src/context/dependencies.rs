//! Small source signatures shared by conversations working for the same accounts.
//! Directory changes compare these inputs; they never read context bodies.

use super::parts::{COUNT, Part};
use crate::Node;
use fastctx_types::{TownName, access::Acting, residents::About};
use serde::Serialize;
use std::collections::BTreeMap;

pub(crate) fn hash(value: &impl Serialize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(value).expect("context inputs serialize").hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone)]
struct AccountInputs {
    authority: u64,
    presence: u64,
    plan: u64,
    user: u64,
    skills: u64,
}

#[derive(Default)]
pub(crate) struct Scan {
    accounts: BTreeMap<String, AccountInputs>,
    projects: BTreeMap<(String, TownName), u64>,
    routes: BTreeMap<u64, u64>,
}

impl Scan {
    pub(crate) fn inputs(&mut self, node: &Node, about: &About, town: Option<&TownName>) -> [u64; COUNT] {
        let account = self
            .accounts
            .entry(serde_json::to_string(&about.acting).expect("acting serializes"))
            .or_insert_with(|| accounts(node, &about.acting));
        let focus = hash(&crate::focus::allowed(node, &about.acting, &about.focus));
        let identity = hash(&(node.machine(), &about.acting, &about.origin, &about.focus));
        let project = town.map(|town| {
            let key = (serde_json::to_string(&about.acting).expect("acting serializes"), town.clone());
            *self.projects.entry(key).or_insert_with(|| node.contexts.projects.signature(node, &about.acting, town))
        });
        let guide_route = node.contexts.routes.signature(node, about, town, Part::Guide, &mut self.routes);
        let history_route = node.contexts.routes.signature(node, about, town, Part::History, &mut self.routes);
        let authors = node.contexts.routes.authors(node, &about.id, &about.acting);
        let mut inputs = [0; COUNT];
        inputs[Part::Focus as usize] = focus;
        inputs[Part::User as usize] = account.user;
        inputs[Part::Project as usize] = hash(&project);
        inputs[Part::Guide as usize] = if about.world_managed() {
            hash(&(identity, account.plan, guide_route.unwrap_or_else(|| hash(&(account.authority, account.presence)))))
        } else {
            0
        };
        inputs[Part::Plan as usize] = hash(&(node.machine(), account.plan));
        inputs[Part::Skills as usize] = account.skills;
        inputs[Part::History as usize] = if fastctx_platform::preview() {
            hash(&(
                identity,
                project,
                account.plan,
                history_route.unwrap_or_else(|| hash(&(account.authority, account.presence))),
                authors.unwrap_or(account.authority),
            ))
        } else {
            0
        };
        inputs
    }
}

fn accounts(node: &Node, acting: &Acting) -> AccountInputs {
    let mut authority = Vec::new();
    let mut presence = Vec::new();
    let mut plan = Vec::new();
    let mut skills = Vec::new();
    for link in node.links() {
        let accounts = node.accounts_on(&link, acting);
        if accounts.is_empty() {
            continue;
        }
        let view = link.view();
        let current = crate::authorization::current_view(&link).is_ok();
        let identities: Vec<_> = accounts
            .iter()
            .map(|name| {
                (
                    name,
                    view.account(name).map(|account| (account.locked, account.opened)),
                    view.may_act_for(name, &view.me),
                )
            })
            .collect();
        let mut machines = Vec::new();
        let mut online = Vec::new();
        for member in view.machines() {
            let rights: Vec<_> = accounts.iter().map(|account| view.rights(account, &member.name).hands).collect();
            if !view.is_me(&member.name) && !rights.iter().any(|right| *right) {
                continue;
            }
            // A machine's other holders also decide which account records it
            // may keep. Their lock status changes that without changing our rights.
            let shares: Vec<_> = view
                .shares
                .iter()
                .filter(|share| share.machine == member.name)
                .map(|share| (&share.account, view.holds(&share.account, &member.name)))
                .collect();
            machines.push(hash(&(
                &member.name,
                &member.was,
                &member.home,
                &member.logins,
                member.managed,
                rights,
                shares,
            )));
            online.push(hash(&(&member.name, member.online, member.since)));
        }
        let base = hash(&(link.hub(), link.hub_name(), &view.me, current, identities));
        authority.push(hash(&(base, machines, view.shared_records, view.plan_copies)));
        presence.push(hash(&(base, online)));
        plan.push(hash(&(base, view.account_records, view.conversation_plans)));
        skills.push(hash(&(base, view.account_records)));
    }
    AccountInputs {
        authority: hash(&(authority, node.works_here(acting).is_ok(), node.links().is_empty())),
        presence: hash(&presence),
        plan: hash(&plan),
        user: hash(&crate::profile::session::prompt(node, acting)),
        skills: hash(&skills),
    }
}
