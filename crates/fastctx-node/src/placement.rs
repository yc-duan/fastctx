//! Resolve account-owned working copies through the same work authority as
//! shell calls. Features supply focus and the speaking device, never a path
//! on the conversation's host as a substitute for an unavailable destination.

pub(crate) mod artifacts;
pub(crate) mod copies;
pub(crate) mod files;
pub(crate) mod jobs;
pub(crate) mod location;
pub(crate) mod ownership;

use crate::Node;
use fastctx_types::{
    MachineName,
    access::{Acting, HubAccount, Written},
    focus::Focus,
    ipc::Failure,
};

#[derive(Clone)]
pub(crate) struct Destination {
    pub machine: Written,
    pub account: HubAccount,
    aliases: Vec<MachineName>,
}

/// The unambiguous account a record belongs to. A caller's previously pinned
/// account remains authoritative only while the conversation still serves it.
pub(crate) fn account(node: &Node, acting: &Acting, pinned: Option<HubAccount>) -> Result<Option<HubAccount>, Failure> {
    if let Some(pinned) = pinned {
        if !node.serves(acting, &pinned.hub, &pinned.account) {
            return Err(Failure::refused("the conversation no longer works for the record's account"));
        }
        return Ok(Some(pinned));
    }
    let mut accounts = match acting {
        Acting::Accounts { accounts } => accounts.clone(),
        Acting::Here => node
            .links()
            .iter()
            .flat_map(|link| {
                node.accounts_on(link, acting)
                    .into_iter()
                    .map(|account| HubAccount { hub: link.membership().id, account })
            })
            .collect(),
    };
    accounts.sort();
    accounts.dedup();
    Ok(match accounts.as_slice() {
        [account] => Some(account.clone()),
        _ => None,
    })
}

/// Storage follows registration ownership, not Hold, which also admits shares.
/// Work authorization is checked separately through the existing shell gate.
pub(crate) fn owned(member: Option<&fastctx_net::directory::Member>, account: &fastctx_types::AccountName) -> bool {
    member.is_some_and(|member| member.home.as_ref() == Some(account) && member.logins.contains(account))
}

/// Current online computers owned by the acting account, excluding machines
/// merely shared with it. Work authorization itself is exactly the shell gate.
pub(crate) fn destinations(node: &Node, acting: &Acting) -> Result<Vec<Destination>, Failure> {
    let mut destinations = Vec::new();
    for link in node.links() {
        let accounts = node.accounts_on(&link, acting);
        if accounts.is_empty() {
            continue;
        }
        let view = crate::authorization::current_view(&link)?;
        for account in accounts {
            if view.account(&account).is_none_or(|account| account.locked) {
                return Err(Failure::refused("the working-copy account is absent or locked"));
            }
            let scope = HubAccount { hub: link.membership().id, account };
            let subject = Acting::one(scope.hub.clone(), scope.account.clone());
            for member in view.machines().filter(|member| member.online && owned(Some(member), &scope.account)) {
                let machine = Written { name: member.name.clone(), hub: Some(link.hub_name()) };
                if node.authorize_machine(&subject, &machine).is_ok() {
                    destinations.push(Destination { machine, account: scope.clone(), aliases: member.was.clone() });
                }
            }
        }
    }
    destinations.sort_by(|a, b| (&a.machine, &a.account).cmp(&(&b.machine, &b.account)));
    destinations.dedup_by(|a, b| a.machine == b.machine && a.account == b.account);
    Ok(destinations)
}

fn matches(wanted: &Written, candidate: &Destination) -> bool {
    (wanted.name == candidate.machine.name || candidate.aliases.contains(&wanted.name))
        && wanted.hub.as_ref().is_none_or(|hub| candidate.machine.hub.as_ref() == Some(hub))
}

/// The account's computers in the order a conversation's working place is
/// chosen (`world-placement.md`, 干活的那台怎样选): its project folders in
/// their recorded order, the computer it lives on, the one its user speaks
/// from, then the account's sole online computer. Only the account's own
/// computers are choices, so a conversation living on someone else's
/// computer passes over it. Ambiguity remains absence; it is never resolved
/// by alphabetical order.
pub(crate) fn ranked<'a>(
    choices: &'a [Destination],
    focus: &Focus,
    residence: Option<&MachineName>,
    speaking: Option<&MachineName>,
) -> Vec<&'a Destination> {
    let named = |name: &MachineName| {
        choices.iter().find(move |choice| choice.machine.name == *name || choice.aliases.contains(name))
    };
    let mut ranked: Vec<&Destination> = Vec::new();
    let found = focus
        .machines
        .iter()
        .filter_map(|focused| choices.iter().find(|choice| matches(&focused.machine, choice)))
        .chain(residence.and_then(named))
        .chain(speaking.and_then(named))
        .chain(match choices {
            [choice] => Some(choice),
            _ => None,
        });
    for choice in found {
        if !ranked.iter().any(|kept| std::ptr::eq(*kept, choice)) {
            ranked.push(choice);
        }
    }
    ranked
}

/// The user's explicit choice, else the first of `ranked`.
pub(crate) fn select<'a>(
    choices: &'a [Destination],
    focus: &Focus,
    residence: Option<&MachineName>,
    speaking: Option<&MachineName>,
    preferred: Option<&Written>,
) -> Result<Option<&'a Destination>, Failure> {
    if let Some(preferred) = preferred {
        return choices
            .iter()
            .find(|choice| matches(preferred, choice))
            .map(Some)
            .ok_or_else(|| Failure::refused("choose one of this account's online machines for files"));
    }
    Ok(ranked(choices, focus, residence, speaking).into_iter().next())
}

/// Every resource adapter enters here before touching a working copy.
pub(crate) async fn require_copy(
    node: &std::sync::Arc<Node>,
    asker: &crate::admission::Asker,
    owner: &fastctx_types::Address,
    placement: u64,
) -> Result<(std::path::PathBuf, fastctx_types::Address), Failure> {
    let account = HubAccount { hub: asker.hub.clone(), account: asker.account.clone() };
    let link = node.link_of(&account.hub).ok_or_else(|| Failure::new("the working-copy hub is unavailable"))?;
    let acting = Acting::one(account.hub.clone(), account.account.clone());
    node.authorize_machine(&acting, &node.machine().into())?;
    if !owned(crate::authorization::current_view(&link)?.member(&node.machine()), &account.account) {
        return Err(Failure::refused("working copies belong on the account's own computers"));
    }
    let (version, place) = location::read_place(&link, &account, owner).await?;
    let place = place.ok_or_else(|| Failure::not_run("the working-copy location was removed"))?;
    if version != Some(placement) || place.machine.as_ref().is_none_or(|machine| !node.is_me(machine)) {
        return Err(Failure::not_run("this is no longer the conversation's working computer"));
    }
    let directory = node.world.owner(&node.names(), &place.owner);
    fastctx_platform::ensure_private_dir(directory.path()).map_err(|error| Failure::new(error.to_string()))?;
    let owner = ownership::bind(node, directory.path(), &account, &place.owner)?;
    Ok((directory.path().to_path_buf(), owner))
}

pub(crate) async fn prepare_copy(
    node: &std::sync::Arc<Node>,
    asker: &crate::admission::Asker,
    owner: fastctx_types::Address,
    placement: u64,
    resource: fastctx_types::placement::Resource,
) -> Result<fastctx_types::placement::Location, Failure> {
    let (directory, owner) = require_copy(node, asker, &owner, placement).await?;
    match resource {
        fastctx_types::placement::Resource::Plan => {
            copies::activate(node, asker, owner, placement).await?;
        }
        fastctx_types::placement::Resource::Skill { name, version } => {
            return crate::skills::copies::prepare(node, asker, Some(&owner), &name, Some(version))
                .await
                .map(|(_, location)| location);
        }
        fastctx_types::placement::Resource::Directory => {}
        fastctx_types::placement::Resource::Files { key, files } => {
            return artifacts::put(node, directory, key, files).await;
        }
    }
    Ok(fastctx_types::placement::Location { machine: node.machine(), path: crate::world::native(&directory) })
}

pub(crate) fn plan_path(location: &fastctx_types::placement::Location) -> String {
    format!("{}/plan.md", location.path.trim_end_matches('/'))
}
