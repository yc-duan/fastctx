//! Allowing direct connections to a machine (`world-network.md`, 直连).
//!
//! Only the super administrator allows them, since they change how the
//! whole network connects. The machine must be connected: it is asked to
//! listen at the chosen addresses' ports first, through its own view of the
//! directory, which shows it the check under way; once it says it listens,
//! and what it knows of its own firewall, the hub connects to every chosen
//! address from where the hub is, as any machine elsewhere would. Direct
//! connections count only when that works; when it does not, the answer
//! says which layer kept the hub out. An address only machines on the same
//! network reach, the hub cannot check from elsewhere, and says so.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use fastctx_net::peer::{Unreached, local_only};
use fastctx_net::wire::{Firewall, Told};
use fastctx_types::machine_log::{Action, Actor, Outcome};
use fastctx_types::{AccountName, MachineName};
use tokio::sync::oneshot;

use crate::Hub;

/// The checks under way, by machine.
#[derive(Default)]
pub(crate) struct Trials {
    by: HashMap<MachineName, Trial>,
}

struct Trial {
    at: Vec<SocketAddr>,
    listening: Option<oneshot::Sender<Result<Firewall, String>>>,
}

impl Hub {
    fn trials(&self) -> std::sync::MutexGuard<'_, Trials> {
        self.trials.lock().expect("never poisoned")
    }

    /// The addresses the hub checks `machine` at now, if it does.
    pub(crate) fn trial_of(&self, machine: &MachineName) -> Vec<SocketAddr> {
        self.trials().by.get(machine).map(|trial| trial.at.clone()).unwrap_or_default()
    }

    /// `machine` says it listens for the check at `at`, or could not.
    pub(crate) fn listening(
        &self,
        machine: &MachineName,
        at: &[SocketAddr],
        outcome: Result<Firewall, String>,
    ) -> Result<Told, String> {
        let mut trials = self.trials();
        let trial = trials.by.get_mut(machine).filter(|trial| trial.at == at).ok_or("no check of it is under way")?;
        if let Some(listening) = trial.listening.take() {
            let _ = listening.send(outcome);
        }
        Ok(Told::Done)
    }
}

/// Allows direct connections to `machine` at `at`, or, with none, no
/// longer: the change `asker` makes as `account`.
pub(crate) async fn change(
    hub: Arc<Hub>,
    asker: MachineName,
    account: AccountName,
    machine: MachineName,
    at: Vec<SocketAddr>,
) -> Result<Told, String> {
    let machine = hub.store().book.directory.canonical(&machine).clone();
    let action = Action::Direct { machine: machine.clone(), at: at.clone() };
    let actor = Actor { account: Some(account.clone()), authenticated: true, member: Some(asker.clone()), peer: None };
    let outcome = allow(&hub, &asker, &account, &machine, &at).await;
    let mut store = hub.store();
    let directory = store.book.directory.clone();
    let kept = match &outcome {
        Ok(_) => crate::machine_log::keep(&mut store, &directory, actor, action, Outcome::Done),
        Err(reason) => crate::machine_log::keep(
            &mut store,
            &directory,
            Actor { authenticated: directory.logged_in(&asker, &account), ..actor },
            action,
            Outcome::Refused { reason: reason.clone() },
        ),
    };
    drop(store);
    match &outcome {
        Ok(_) if at.is_empty() => hub.log.line(format_args!("{machine} no longer takes direct connections")),
        Ok(_) => hub.log.line(format_args!(
            "{machine} takes direct connections at {}",
            at.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        )),
        Err(reason) => hub.log.line(format_args!("direct connections to {machine} were not allowed: {reason}")),
    }
    hub.changed();
    kept?;
    outcome
}

fn authorized(
    directory: &fastctx_net::directory::Directory,
    asker: &MachineName,
    account: &AccountName,
    machine: &MachineName,
    at: &[SocketAddr],
) -> Result<fastctx_net::sign::PublicKey, String> {
    if !directory.logged_in(asker, account) {
        return Err(format!("{asker} is not logged in to the account {account}"));
    }
    if directory.account(account).is_some_and(|found| found.locked) {
        return Err(format!("the account {account} is locked"));
    }
    if !directory.is_super(account) {
        return Err(format!("only the network's administrator allows direct connections, and {account} is not it"));
    }
    let member = directory.member(machine).ok_or_else(|| format!("there is no machine named {machine}"))?;
    if !member.is_machine() {
        return Err(format!("{machine} is a device, not a machine; only machines take direct connections"));
    }
    // A visitor's machines are its alone, the administrator's reach
    // stops there; one that takes them already may still be stopped.
    if !at.is_empty() && member.home.as_ref().is_some_and(|owner| directory.is_visitor(owner)) {
        return Err(format!("{machine} is a visitor's machine, which only its owner uses"));
    }
    Ok(member.key)
}

async fn allow(
    hub: &Arc<Hub>,
    asker: &MachineName,
    account: &AccountName,
    machine: &MachineName,
    at: &[SocketAddr],
) -> Result<Told, String> {
    let key = authorized(&hub.store().book.directory, asker, account, machine, at)?;
    if at.is_empty() {
        let mut store = hub.store();
        if authorized(&store.book.directory, asker, account, machine, at)? != key {
            return Err("the machine's identity changed while direct connections were being changed".into());
        }
        let member = store
            .book
            .directory
            .members
            .iter_mut()
            .find(|member| member.name == *machine)
            .map(Arc::make_mut)
            .ok_or_else(|| format!("the machine {machine} was removed"))?;
        member.direct.clear();
        store.save_book().map_err(|error| format!("keeping the change: {error}"))?;
        return Ok(Told::Done);
    }
    match hub.sessions().get(machine) {
        None => {
            return Err(format!(
                "{machine} is offline; it must be connected to listen while the hub checks it reaches it"
            ));
        }
        Some(session) if !session.direct => {
            return Err(format!("{machine} runs an older fastctx that cannot take direct connections"));
        }
        Some(_) => {}
    }
    let (listening, ready) = oneshot::channel();
    {
        let mut trials = hub.trials();
        if trials.by.contains_key(machine) {
            return Err(format!("the hub is checking {machine} already"));
        }
        trials.by.insert(machine.clone(), Trial { at: at.to_vec(), listening: Some(listening) });
    }
    struct Done<'a>(&'a Hub, &'a MachineName);
    impl Drop for Done<'_> {
        fn drop(&mut self) {
            self.0.trials().by.remove(self.1);
        }
    }
    let _done = Done(hub, machine);
    // The machine sees the check in its view, listens, and says so.
    hub.changed();
    // A machine that stays connected and never says so holds the check no
    // longer than this.
    const LISTENS_WITHIN: std::time::Duration = std::time::Duration::from_secs(60);
    let firewall = tokio::select! {
        ready = ready => ready.map_err(|_| format!("{machine} left before it said it listens"))?,
        () = hub.gone(machine) => return Err(format!("{machine} disconnected before it said it listens")),
        () = tokio::time::sleep(LISTENS_WITHIN) => {
            return Err(format!("{machine} did not say it listens within {} s", LISTENS_WITHIN.as_secs()));
        }
    };
    let firewall = firewall.map_err(|why| format!("{machine} could not take direct connections: {why}"))?;
    let hub_id = hub.store().hub.id.clone();
    let probes = at.iter().map(|address| {
        let (hub_id, machine) = (hub_id.clone(), machine.clone());
        async move { (*address, fastctx_net::peer::probe(*address, &hub_id, &machine, &key).await) }
    });
    let mut failures = Vec::new();
    for (address, outcome) in futures_util::future::join_all(probes).await {
        match outcome {
            Ok(()) => {}
            // Only machines on its own network reach such an address; the
            // hub, elsewhere, cannot check it.
            Err(Unreached::Silent | Unreached::Refused(_)) if local_only(&address.ip()) => {}
            Err(unreached) => failures.push(explain(machine, address, &unreached, &firewall)),
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    let mut store = hub.store();
    if authorized(&store.book.directory, asker, account, machine, at)? != key {
        return Err("the machine's identity changed while its direct address was checked".into());
    }
    let member = store
        .book
        .directory
        .members
        .iter_mut()
        .find(|member| member.name == *machine)
        .map(Arc::make_mut)
        .ok_or_else(|| format!("the machine {machine} was removed"))?;
    member.direct = at.to_vec();
    store.save_book().map_err(|error| format!("keeping the change: {error}"))?;
    Ok(Told::Done)
}

/// Why the hub did not reach `machine` at `address`, by the layer that kept
/// it out, as far as what the machine knows of its own firewall tells.
fn explain(machine: &MachineName, address: SocketAddr, unreached: &Unreached, firewall: &Firewall) -> String {
    let port = address.port();
    match unreached {
        Unreached::Silent => {
            let layer = match firewall {
                Firewall::Opened => format!(
                    "{machine}'s own firewall lets TCP port {port} in, so the port is closed before it reaches the machine: open TCP port {port} in the cloud provider's security group, or forward it on the router"
                ),
                Firewall::Other { name } => format!(
                    "either {machine}'s own firewall ({name}) or the cloud provider's security group drops TCP port {port}: let it in on both"
                ),
                Firewall::NoneFound => format!(
                    "{machine} runs no firewall the World knows of, so TCP port {port} is most likely closed by the cloud provider's security group or a router: open it there"
                ),
                Firewall::Unknown { why } => format!(
                    "either {machine}'s own firewall ({why}) or the cloud provider's security group drops TCP port {port}: let it in on both"
                ),
            };
            format!("the hub could not reach {machine} at {address}: {unreached}; {layer}")
        }
        Unreached::Refused(_) => format!(
            "the hub could not reach {machine} at {address}: {unreached}; nothing listens there for {machine}: the address may lead to another computer, such as a router that does not forward port {port}, or a firewall on the way rejects it"
        ),
        Unreached::Other(_) => format!(
            "the hub did not reach {machine} at {address}: {unreached}; the address may lead to another computer or program, such as a router forwarding port {port} elsewhere, or a proxy where the hub runs answers for every address"
        ),
    }
}
