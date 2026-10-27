//! Asking every machine something at once (`world-town.md`, `fastctx w`):
//! the machines whoever asks may use, on every hub this computer is on,
//! each asked for the accounts it works for, the online ones at once.

use std::sync::Arc;

pub(crate) use fastctx_net::around::{Heard, Reachable, ask_each};
use fastctx_net::link::{Link, Status};
use fastctx_types::access::{Acting, Purpose};

use crate::Node;

/// The other machines `acting` may ask `purpose` of, on every hub. A
/// computer on several of them is asked once, through the first hub where
/// `acting` may ask it, one this computer is connected to before one it is
/// not.
pub(crate) fn reachable(node: &Node, acting: &Acting, purpose: Purpose) -> Vec<Reachable> {
    let mut reachable = Vec::new();
    let mut keys = Vec::new();
    let links = node.links();
    let mut order: Vec<&Arc<Link>> = links.iter().collect();
    order.sort_by_key(|link| !matches!(link.status(), Status::Online { .. }));
    for link in order {
        let Some(view) = link.last_view() else { continue };
        let accounts = node.accounts_on(link, acting);
        // This computer as its hub calls it, which a hub that has not
        // followed a rename yet calls it otherwise.
        let here = &view.me;
        for machine in view.machines().filter(|machine| !view.is_me(&machine.name)) {
            if keys.contains(&machine.key) {
                continue;
            }
            if let Some(account) = view.account_for(&accounts, here, &machine.name, purpose) {
                keys.push(machine.key);
                let written = fastctx_net::around::written_among(&links, link, &machine.name);
                reachable.push(Reachable {
                    link: link.clone(),
                    name: machine.name.clone(),
                    account,
                    key: machine.key,
                    written,
                });
            }
        }
    }
    reachable
}

/// As `reachable`, but each machine once for every account `acting` works
/// for that may ask `purpose` of it there. A machine shows each account the
/// conversations that work for it alone (`admission::seen`), so a computer
/// logged in to several accounts asks as each on every hub and merges what
/// it hears by hub and machine key.
pub(crate) fn reachable_each(node: &Node, acting: &Acting, purpose: Purpose) -> Vec<Reachable> {
    let mut reachable = Vec::new();
    let links = node.links();
    for link in &links {
        let Some(view) = link.last_view() else { continue };
        let accounts = node.accounts_on(link, acting);
        for machine in view.machines().filter(|machine| !view.is_me(&machine.name)) {
            for account in &accounts {
                if let Some(account) = view.account_for(std::slice::from_ref(account), &view.me, &machine.name, purpose)
                    && !reachable.iter().any(|asked: &Reachable| {
                        asked.link.membership().id == view.hub && asked.key == machine.key && asked.account == account
                    })
                {
                    reachable.push(Reachable {
                        link: link.clone(),
                        name: machine.name.clone(),
                        account,
                        key: machine.key,
                        written: fastctx_net::around::written_among(&links, link, &machine.name),
                    });
                }
            }
        }
    }
    reachable
}

/// Whom whoever makes a local call works for: a resident's own standing,
/// or this computer's logins for the user and pages.
pub(crate) fn acting_of(node: &Node, caller: &crate::town::caller::Caller) -> Acting {
    match caller {
        crate::town::caller::Caller::Resident(address) if node.is_me(&address.machine) => {
            crate::here::acting(node, &address.resident).unwrap_or(Acting::Accounts { accounts: Vec::new() })
        }
        // A resident of another machine runs commands here only in a job
        // that machine sent for an account: they work for that account, and
        // never for the ones this computer is logged in to
        // (`world-network.md`, 一个登录，只在登录它的那台电脑上用).
        crate::town::caller::Caller::Resident(address) => {
            Acting::Accounts { accounts: crate::jobs::running_for(&node.world, &node.names(), address) }
        }
        // A page works for the account that agreed to what it does.
        crate::town::caller::Caller::Page(page) => {
            crate::pages::identity::acting(node, page).unwrap_or_else(|_| Acting::Accounts { accounts: Vec::new() })
        }
        crate::town::caller::Caller::User => Acting::Here,
        crate::town::caller::Caller::Session(_) => Acting::Accounts { accounts: Vec::new() },
    }
}

/// Whether whoever makes a local call works on this computer: as the
/// accounts it works for may, wherever its session lives; an agent
/// assisting another account from its owner's computer does not
/// (`world-network.md`, 共享机器与智能体协助).
pub(crate) fn works_here(node: &Node, acting: &Acting) -> bool {
    node.works_here(acting).is_ok()
}

/// A session may see its conversations at home even when it only assists
/// an account and cannot execute commands on that machine.
pub(crate) fn looks_here(node: &Node, acting: &Acting) -> bool {
    works_here(node, acting)
        || node.links().iter().any(|link| {
            let view = link.view();
            node.accounts_on(link, acting)
                .iter()
                .any(|account| view.admits(&view.me, account, &view.me, Purpose::Look).is_ok())
        })
}
