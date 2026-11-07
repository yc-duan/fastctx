//! Keep the scope of record notifications until their context refresh is queued.

use super::parts::{Part, Parts};
use crate::Node;
use fastctx_types::{
    ResidentId, TownName,
    access::{HubId, TownId},
    account_records::Changed,
    history::Scope,
    replicas::Document,
    residents::Origin,
};

pub(crate) fn request_plan(node: &Node, id: &ResidentId) {
    node.contexts.request(id, Parts::of(&[Part::Guide, Part::Plan]));
}

pub(crate) fn project(node: &Node, town: &TownName) {
    node.contexts.projects.invalidate(town);
    for resident in
        node.residents.everyone().filter(|resident| resident.removed.is_none() && resident.town.as_ref() == Some(town))
    {
        node.contexts.request(&resident.id, Parts::of(&[Part::Project, Part::History]));
    }
}

pub(crate) fn history_at(node: &Node, hub: &HubId, town: &TownId) {
    let mut names = std::collections::BTreeSet::new();
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        if let Some(name) = resident.town {
            names.insert(name);
        }
    }
    for project in names.into_iter().filter(|name| {
        crate::ledger::names::place(&node.world, name)
            .ok()
            .flatten()
            .is_some_and(|place| place.hub == *hub && place.town == *town)
    }) {
        history(node, &Scope::Project { project });
    }
}

pub(crate) fn history(node: &Node, scope: &Scope) {
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        let relevant = match scope {
            Scope::Project { project } => resident.town.as_ref() == Some(project),
            Scope::Conversation { resident: owner, machine } => match &resident.about.origin {
                Origin::User => resident.id == *owner && node.is_me(machine),
                Origin::Colleague { asked_by, .. } => {
                    asked_by.resident == *owner
                        && (asked_by.machine == *machine
                            || node.links().iter().any(|link| link.view().same(&asked_by.machine, machine)))
                }
            },
        };
        if relevant {
            node.contexts.request(&resident.id, Parts::of(&[Part::History]));
        }
    }
}

pub(crate) fn hub(node: &Node, hub: &HubId) {
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        if relevant(node, &resident.about.acting, hub) {
            node.contexts.request(&resident.id, Parts::of(&[Part::Guide, Part::History]));
        }
    }
}

pub(crate) fn reconnected(node: &Node, hub: &HubId) {
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        if relevant(node, &resident.about.acting, hub) {
            node.plan_reads.invalidate(&resident.id);
            node.contexts.request(&resident.id, Parts::of(&[Part::Guide, Part::Plan, Part::Skills, Part::History]));
        }
    }
}

fn relevant(node: &Node, acting: &fastctx_types::access::Acting, hub: &HubId) -> bool {
    (match acting {
        fastctx_types::access::Acting::Here => true,
        fastctx_types::access::Acting::Accounts { accounts } => accounts.iter().any(|account| account.hub == *hub),
    }) && node.link_of(hub).is_some()
}

pub(crate) fn record(node: &Node, hub: &HubId, change: &Changed) {
    let kind = change.metadata.kind.as_str();
    let ownership = kind == fastctx_types::owned_records::KIND;
    let owner = if ownership {
        change.metadata.key.as_str()
    } else if kind == fastctx_types::history::RECORD_KIND {
        let Some((owner, _)) = change.metadata.key.split_once('/') else {
            return;
        };
        owner
    } else {
        return;
    };
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        let owner_id = match &resident.about.origin {
            Origin::User => &resident.id,
            Origin::Colleague { asked_by, .. } => &asked_by.resident,
        };
        if (owner_id.as_str() == owner || (ownership && resident.id.as_str() == owner))
            && relevant(node, &resident.about.acting, hub)
        {
            let parts = if ownership {
                Parts::of(&[Part::Guide, Part::Plan, Part::History])
            } else {
                Parts::of(&[Part::History])
            };
            node.contexts.request(&resident.id, parts);
        }
    }
}

pub(crate) fn document(node: &Node, hub: &HubId, document: &Document) {
    match document {
        Document::Plan { owner } => {
            if node.is_me(&owner.machine) {
                request_plan(node, &owner.resident);
            }
        }
        Document::History { scope, .. } => match scope {
            fastctx_types::replicas::Scope::Project { town, .. } => history_at(node, hub, town),
            fastctx_types::replicas::Scope::Conversation { conversation } => history(
                node,
                &Scope::Conversation {
                    resident: conversation.address.resident.clone(),
                    machine: conversation.address.machine.clone(),
                },
            ),
        },
    }
}
