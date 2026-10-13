//! Asking every machine something at once (`world-town.md`, `fastctx w`):
//! the machines whoever asks may use, on every hub this computer is on,
//! each asked for one of the accounts it works for, the online ones at once.

use std::sync::Arc;

use fastctx_net::link::{CallFailure, Link};
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;

use crate::Node;

/// How one machine answered.
pub(crate) enum Heard {
    Answered(RemoteReply),
    Offline(Option<Timestamp>),
    NoAnswer(String),
}

/// A machine to ask, the hub it is asked through, and the account to ask
/// it for.
pub(crate) struct Reachable {
    pub link: Arc<Link>,
    pub name: MachineName,
    pub account: AccountName,
}

/// The other machines `acting` may ask `purpose` of, on every hub. A
/// computer on several of them is asked once, through the first hub where
/// `acting` may ask it.
pub(crate) fn reachable(node: &Node, acting: &Acting, purpose: Purpose) -> Vec<Reachable> {
    let mut reachable = Vec::new();
    let mut keys = Vec::new();
    for link in node.links() {
        let view = link.view();
        let accounts = node.accounts_on(&link, acting);
        // This computer as its hub calls it, which a hub that has not
        // followed a rename yet calls it otherwise.
        let here = &view.me;
        for machine in view.machines().filter(|machine| !view.is_me(&machine.name)) {
            if keys.contains(&machine.key) {
                continue;
            }
            if let Some(account) = view.account_for(&accounts, here, &machine.name, purpose) {
                keys.push(machine.key);
                reachable.push(Reachable { link: link.clone(), name: machine.name.clone(), account });
            }
        }
    }
    reachable
}

/// Asks each of `machines` at once, the online ones by the call `call`
/// makes for each; the others say why they were not asked.
pub(crate) async fn ask_each(
    machines: Vec<Reachable>,
    call: impl Fn(&MachineName) -> RemoteCall,
) -> Vec<(MachineName, Heard)> {
    let mut heard = Vec::new();
    let mut asks = Vec::new();
    let mut refreshed: Vec<(Arc<Link>, Result<(), String>)> = Vec::new();
    for Reachable { link, name, account } in machines {
        // Who is connected, as each hub has it now: asked once per hub.
        let fresh = match refreshed.iter().find(|(asked, _)| Arc::ptr_eq(asked, &link)) {
            Some((_, fresh)) => fresh.clone(),
            None => {
                let fresh = link.refresh().await;
                refreshed.push((link.clone(), fresh.clone()));
                fresh
            }
        };
        if let Err(error) = fresh {
            let reason = format!("whether it is connected is not known: the hub could not be asked ({error})");
            heard.push((name, Heard::NoAnswer(reason)));
            continue;
        }
        let seen = link.view().member(&name).cloned();
        match seen {
            Some(seen) if seen.online => {
                let call = call(&name);
                asks.push((
                    name.clone(),
                    tokio::spawn(async move {
                        let purpose = call.purpose();
                        match link.call(&name, &account, purpose, call).await {
                            Ok(Ok(reply)) => Heard::Answered(reply),
                            Ok(Err(failure)) => Heard::NoAnswer(failure.message),
                            Err(CallFailure::Offline { last_seen, .. }) => Heard::Offline(last_seen),
                            Err(failure) => Heard::NoAnswer(failure.to_string()),
                        }
                    }),
                ));
            }
            Some(seen) => heard.push((name, Heard::Offline(seen.since))),
            None => heard.push((name, Heard::NoAnswer("the hub did not say whether it is connected".to_string()))),
        }
    }
    for (name, ask) in asks {
        heard.push(match ask.await {
            Ok(answer) => (name, answer),
            Err(error) => (name, Heard::NoAnswer(format!("asking around: {error}"))),
        });
    }
    heard.sort_by(|a, b| a.0.cmp(&b.0));
    heard
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
