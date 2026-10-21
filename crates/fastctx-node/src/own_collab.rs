//! This computer's own model collaboration (`world-colleagues.md`, 同事从哪里
//! 来、能去哪里).
//!
//! A computer in no network has no account to keep the settings on, so it
//! keeps them beside its World, and its callers and callees are its own
//! logins. When it is first registered as a machine of an account, they go
//! into that account: whole when the account has none yet; otherwise the
//! account keeps its switch, its text for callers and its rules, and this
//! computer's callers and callees join its own, a callee whose name the
//! account already uses taking the first free `name-N`. What it kept stays
//! beside the World as `collab.carried.json`. When it leaves its last
//! network, the settings of the account it was registered under, as far as
//! they name its own logins, become its own again.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_net::ids::HubId;
use fastctx_net::link::Link;
use fastctx_types::MachineName;
use fastctx_types::collab::{AgentLogin, Collab};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;

use crate::Node;

/// What keeps writes and carrying one at a time.
#[derive(Default)]
pub(crate) struct Keeping {
    writing: std::sync::Mutex<()>,
    carrying: crate::ranked::Carrying,
}

/// What this computer kept once its settings went into an account.
fn carried_path(world: &WorldDir) -> PathBuf {
    world.path().join("collab.carried.json")
}

/// This computer's own settings; all off and empty until written.
pub(crate) fn read(world: &WorldDir) -> Result<Collab, Failure> {
    read_at(&world.own_collab())
        .map(Option::unwrap_or_default)
        .map_err(|error| Failure::new(format!("reading {}: {error}", world.own_collab().display())))
}

fn read_at(path: &Path) -> std::io::Result<Option<Collab>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn keep(path: &Path, collab: &Collab) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(collab).expect("collaboration serializes");
    fastctx_platform::publish(path, &bytes, fastctx_platform::Permissions::Private)
}

/// Writes this computer's own settings over the version they were read at;
/// the new version. Only while it is in no network: in one, model
/// collaboration is its accounts'.
pub(crate) fn write(node: &Node, mut collab: Collab) -> Result<u64, Failure> {
    if node.connected_anywhere() {
        return Err(Failure::not_run(
            "this computer is in a network, where model collaboration is set for each account; name the account",
        ));
    }
    collab.validate().map_err(Failure::not_run)?;
    let here = node.machine();
    let logins = collab.callers.iter().chain(collab.callees.iter().map(|callee| &callee.login));
    if let Some(other) = logins.map(|login| &login.machine).find(|machine| !node.is_me(machine)) {
        return Err(Failure::not_run(format!(
            "{other} is not this computer, {here}: a computer in no network calls and is called only by its own logins"
        )));
    }
    let _writing = node.own_collab.writing.lock().expect("never poisoned");
    let current = read(&node.world)?;
    if collab.version != current.version {
        return Err(Failure::not_run(format!(
            "model collaboration changed: version {} was read, current version is {}",
            collab.version, current.version
        )));
    }
    collab.version = current.version + 1;
    let path = node.world.own_collab();
    keep(&path, &collab).map_err(|error| Failure::new(format!("writing {}: {error}", path.display())))?;
    node.tools_changed(None);
    Ok(collab.version)
}

/// Whether two logins on this computer are the same: an unnamed directory
/// is the agent's default one.
fn same_login(node: &Node, a: &AgentLogin, b: &AgentLogin) -> Result<bool, Failure> {
    Ok(a.machine == b.machine
        && a.agent == b.agent
        && crate::collab::matches_home(node, &a.agent, a.home.as_deref(), b.home.as_deref())?)
}

/// `collab` with every login named as this computer is now called: its own
/// settings name no other machine, and the computer may have taken another
/// name when it logged in.
fn renamed(mut collab: Collab, here: &MachineName) -> Collab {
    for caller in &mut collab.callers {
        caller.machine = here.clone();
    }
    for callee in &mut collab.callees {
        callee.login.machine = here.clone();
    }
    collab
}

/// The account's settings with this computer's own in them, and what that
/// came to, in a line for the log.
fn merge(node: &Node, account: Option<Collab>, own: Collab) -> Result<(Collab, String), Failure> {
    let Some(mut merged) = account.filter(|account| account.version > 0) else {
        let line = format!("whole: {} callers, {} callees", own.callers.len(), own.callees.len());
        return Ok((Collab { version: 0, ..own }, line));
    };
    let mut added = (0, 0);
    let mut renamed = Vec::new();
    for caller in own.callers {
        let mut present = false;
        for existing in &merged.callers {
            present |= same_login(node, existing, &caller)?;
        }
        if !present {
            merged.callers.push(caller);
            added.0 += 1;
        }
    }
    for mut callee in own.callees {
        let mut present = false;
        for existing in &merged.callees {
            present |= existing.name == callee.name
                && existing.model == callee.model
                && existing.effort == callee.effort
                && same_login(node, &existing.login, &callee.login)?;
        }
        if present {
            continue;
        }
        if merged.callees.iter().any(|existing| existing.name == callee.name) {
            let name = (2..)
                .map(|n| format!("{}-{n}", callee.name))
                .find(|name| merged.callees.iter().all(|existing| existing.name != *name))
                .expect("a free name");
            renamed.push(format!("{} as {name}", callee.name));
            callee.name = name;
        }
        merged.callees.push(callee);
        added.1 += 1;
    }
    let mut line = format!(
        "added {} callers and {} callees; the account's switch, text for callers and rules stay",
        added.0, added.1
    );
    if !renamed.is_empty() {
        line.push_str(&format!("; renamed {}", renamed.join(", ")));
    }
    Ok((merged, line))
}

/// Carries this computer's own settings into the account it is registered
/// under at `hub`, when it keeps any; each step is logged. Run when it
/// logs in and each time its link connects, until they are carried.
pub(crate) async fn carry(node: &Arc<Node>, hub: &HubId) {
    // A managed machine is logged in to no account to carry it to.
    if crate::managed::is_managed(node) {
        return;
    }
    let _carrying = node.own_collab.carrying.lock().await;
    let path = node.world.own_collab();
    let own = match read_at(&path) {
        Ok(Some(own)) => own,
        Ok(None) => return,
        Err(error) => {
            node.log.line(format_args!("reading {}: {error}", path.display()));
            return;
        }
    };
    let Some(link) = node.link_of(hub) else { return };
    if let Err(error) = link.refresh().await {
        node.log.line(format_args!("carrying this computer's model collaboration: asking the hub: {error}"));
        return;
    }
    let view = link.view();
    // Logged in without being registered, the computer is no account's
    // machine; its settings wait for the account it is registered under.
    let Some(home) = view.me().and_then(|me| me.home.clone()) else { return };
    let account = view.account(&home).and_then(|found| found.collab.clone());
    let (merged, line) = match merge(node, account, renamed(own, &node.machine())) {
        Ok(merged) => merged,
        Err(failure) => {
            node.log.line(format_args!("carrying this computer's model collaboration: {}", failure.message));
            return;
        }
    };
    if let Err(error) = link.collab_write(&home, merged).await {
        node.log.line(format_args!("carrying this computer's model collaboration into {home}: {error}"));
        return;
    }
    if let Err(error) = std::fs::rename(&path, carried_path(&node.world)) {
        node.log.line(format_args!(
            "this computer's model collaboration is in {home}, but putting {} aside failed: {error}",
            path.display()
        ));
        return;
    }
    node.log.line(format_args!(
        "this computer's model collaboration went into {home} on {}: {line}; what it kept is in {}",
        link.hub_name(),
        carried_path(&node.world).display()
    ));
}

/// As this computer leaves its last network through `link`: the settings of
/// the account it was registered under there, as far as they name its own
/// logins, become its own, unless it still keeps some it never carried.
pub(crate) fn take_back(node: &Node, link: &Link) {
    let path = node.world.own_collab();
    match path.try_exists() {
        Ok(false) => {}
        Ok(true) => return,
        Err(error) => {
            node.log.line(format_args!("taking model collaboration back: {}: {error}", path.display()));
            return;
        }
    }
    let view = link.view();
    let Some(home) = view.me().and_then(|me| me.home.clone()) else { return };
    let Some(collab) = view.account(&home).and_then(|found| found.collab.clone()) else { return };
    let here = node.machine();
    let own = Collab {
        callers: collab.callers.into_iter().filter(|caller| view.is_me(&caller.machine)).collect(),
        callees: collab.callees.into_iter().filter(|callee| view.is_me(&callee.login.machine)).collect(),
        ..collab
    };
    match keep(&path, &own) {
        Ok(()) => node.log.line(format_args!(
            "model collaboration of {home} as it names {here} is this computer's own now: {} callers, {} callees",
            own.callers.len(),
            own.callees.len()
        )),
        Err(error) => node.log.line(format_args!(
            "this computer left its last network, but keeping {home}'s model collaboration as its own failed: {}: {error}",
            path.display()
        )),
    }
}
