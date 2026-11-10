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
    /// The account's prompts, each family's read from them.
    user: fastctx_types::profile::Profile,
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
        self.read_inputs(node, about, town, false)
    }

    pub(super) fn observed_inputs(&mut self, node: &Node, about: &About, town: Option<&TownName>) -> [u64; COUNT] {
        self.read_inputs(node, about, town, true)
    }

    fn read_inputs(&mut self, node: &Node, about: &About, town: Option<&TownName>, observing: bool) -> [u64; COUNT] {
        let account = self
            .accounts
            .entry(serde_json::to_string(&about.acting).expect("acting serializes"))
            .or_insert_with(|| accounts(node, &about.acting));
        let focus = focus(node, about);
        let identity = hash(&(node.machine(), &about.acting, &about.origin, &about.focus));
        let project = town.map(|town| {
            let key = (serde_json::to_string(&about.acting).expect("acting serializes"), town.clone());
            *self.projects.entry(key).or_insert_with(|| {
                if observing {
                    node.contexts.projects.observed(node, &about.acting, town)
                } else {
                    node.contexts.projects.signature(node, &about.acting, town)
                }
            })
        });
        let guide_route = node.contexts.routes.signature(node, about, town, Part::Guide, &mut self.routes);
        let history_route = node.contexts.routes.signature(node, about, town, Part::History, &mut self.routes);
        let authors = node.contexts.routes.authors(node, &about.id, &about.acting);
        let mut inputs = [0; COUNT];
        inputs[Part::Focus as usize] = focus;
        inputs[Part::User as usize] = hash(&account.user.prompt_for(about.host.agent.as_str()));
        inputs[Part::Project as usize] = hash(&project);
        inputs[Part::Guide as usize] = if about.world_managed() {
            hash(&(identity, account.plan, guide_route.unwrap_or_else(|| hash(&(account.authority, account.presence)))))
        } else {
            0
        };
        inputs[Part::Plan as usize] = hash(&(node.machine(), account.plan));
        inputs[Part::Skills as usize] = account.skills;
        inputs[Part::History as usize] = if crate::history::offered::to(node, about) {
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

fn focus(node: &Node, about: &About) -> u64 {
    let focus = crate::focus::allowed(node, &about.acting, &about.focus);
    let ways: Vec<_> = focus
        .machines
        .iter()
        .filter(|machine| !machine.folders.is_empty() && !node.is_here(&machine.machine))
        .flat_map(|machine| {
            node.locate(&machine.machine).unwrap_or_default().into_iter().map(move |link| {
                let way = link.way_to(&machine.machine.name).map(|way| match way {
                    fastctx_net::link::Way::Hub { line, since } => (0, line, since),
                    fastctx_net::link::Way::Direct { link } => (1, link, None),
                });
                (link.hub().clone(), way)
            })
        })
        .collect();
    hash(&(focus, ways))
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
        let index = fastctx_net::directory_index::Index::new(&view);
        let indexed = fastctx_net::directory_index::Indexed::new(&view, &index);
        let current = crate::authorization::current_view(&link).is_ok();
        let identities: Vec<_> = accounts
            .iter()
            .map(|name| {
                (
                    name,
                    indexed.account(name).map(|account| (account.locked, account.opened)),
                    indexed.may_act_for(name, &view.me),
                )
            })
            .collect();
        let mut machines = Vec::new();
        let mut online = Vec::new();
        for member in view.machines() {
            let rights: Vec<_> = accounts
                .iter()
                .map(|account| {
                    indexed.authorize(account, fastctx_net::authorization::Action::Work(&member.name)).is_ok()
                })
                .collect();
            if !view.is_me(&member.name) && !rights.iter().any(|right| *right) {
                continue;
            }
            // A machine's other holders also decide which account records it
            // may keep. Their lock status changes that without changing our rights.
            let shares: Vec<_> = indexed
                .shares_by_machine(&member.name)
                .iter()
                .map(|position| &view.shares[*position])
                .map(|share| (&share.account, indexed.holds(&share.account, &member.name)))
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
        user: {
            let profile = crate::profile::session::get(node, acting);
            fastctx_types::profile::Profile { prompt: profile.prompt, prompts: profile.prompts, ..Default::default() }
        },
        skills: hash(&skills),
    }
}
