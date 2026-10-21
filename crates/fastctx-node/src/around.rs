//! Asking every machine something at once (`world-town.md`, `fastctx w`):
//! the machines whoever asks may use, on every hub this computer is on,
//! each asked for the accounts it works for, the online ones at once.

pub(crate) use fastctx_net::around::{Heard, Reachable, ask_each};
use fastctx_types::access::{Acting, Purpose};

use crate::Node;

/// The other machines `acting` may ask `purpose` of, on every hub. A
/// computer on several of them is asked once, through the first hub where
/// `acting` may ask it.
pub(crate) fn reachable(node: &Node, acting: &Acting, purpose: Purpose) -> Vec<Reachable> {
    let mut reachable = Vec::new();
    let mut keys = Vec::new();
    let links = node.links();
    for link in &links {
        let view = link.view();
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
        let view = link.view();
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
            node.residents.about(&address.resident).map(|(about, _)| about.acting).unwrap_or_default()
        }
        // A resident of another machine runs commands here only in a job
        // that machine sent for an account: they work for that account, and
        // never for the ones this computer is logged in to
        // (`world-network.md`, 一个登录，只在登录它的那台电脑上用).
        crate::town::caller::Caller::Resident(address) => {
            Acting::Accounts { accounts: crate::jobs::running_for(&node.world, &node.names(), address) }
        }
        _ => Acting::Here,
    }
}

/// Whether whoever makes a local call works on this computer: a session
/// living here does, whoever it works for, a login lent to another account
/// among them, as its owner's own sessions do (`world-network.md`,
/// 共享机器与智能体协助); anyone else as the accounts it works for may.
pub(crate) fn works_here(node: &Node, caller: &crate::town::caller::Caller, acting: &Acting) -> bool {
    let lives_here = matches!(caller, crate::town::caller::Caller::Resident(address)
        if node.is_me(&address.machine) && crate::here::lives_here(node, &address.resident));
    lives_here || node.works_here(acting).is_ok()
}
