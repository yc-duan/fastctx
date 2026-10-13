//! What each other machine said of itself (`RemoteCall::Facts`), kept here
//! for interfaces: asked once each time a machine connects anew, and kept
//! on disk beside the hub's records so an interface opened later shows it
//! at once, without asking (`world-network.md`, 近况).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use fastctx_net::directory::Member;
use fastctx_net::link::Link;
use fastctx_types::access::{HubId, Purpose};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::world::MachineFacts;
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

const FILE: &str = "machine-facts.json";

#[derive(Default)]
pub(crate) struct Facts {
    kept: Mutex<HashMap<HubId, BTreeMap<MachineName, Kept>>>,
    asking: Mutex<HashSet<(HubId, MachineName)>>,
}

/// What a machine said of itself, and when it had connected then.
#[derive(Clone, Serialize, Deserialize)]
struct Kept {
    since: Option<Timestamp>,
    facts: Result<MachineFacts, String>,
}

/// What `member` of `link`'s hub said of itself, as kept here. When it is
/// online and has connected since, it is asked again for `account`, and
/// whoever shows the World is told once it answers.
pub(crate) fn of(
    node: &Arc<Node>,
    link: &Arc<Link>,
    member: &Member,
    account: Option<AccountName>,
) -> Result<MachineFacts, String> {
    let hub = link.membership().id.clone();
    let kept = {
        let mut kept = node.facts.kept.lock().expect("never poisoned");
        let machines = kept.entry(hub.clone()).or_insert_with(|| read(node, &hub));
        machines.get(&member.name).cloned()
    };
    let current = kept.as_ref().is_some_and(|kept| kept.since == member.since);
    if member.online
        && !current
        && let Some(account) = account
        && node.facts.asking.lock().expect("never poisoned").insert((hub.clone(), member.name.clone()))
    {
        ask(node, link, hub, member.name.clone(), member.since, account, kept.as_ref().map(|kept| kept.facts.clone()));
    }
    match kept {
        Some(kept) => kept.facts,
        None if member.online => Err("it has not said yet".to_string()),
        None => Err("it is offline".to_string()),
    }
}

fn ask(
    node: &Arc<Node>,
    link: &Arc<Link>,
    hub: HubId,
    name: MachineName,
    since: Option<Timestamp>,
    account: AccountName,
    before: Option<Result<MachineFacts, String>>,
) {
    let (node, link) = (node.clone(), link.clone());
    tokio::spawn(async move {
        let answer = match link.call(&name, &account, Purpose::Look, RemoteCall::Facts).await {
            Ok(Ok(RemoteReply::Facts(facts))) => Ok(facts),
            Ok(Ok(_)) => Err("it answered something else".to_string()),
            Ok(Err(failure)) => Err(failure.message),
            Err(failure) => Err(failure.to_string()),
        };
        // What it said before stands when it could not say it again; it
        // is asked again the next time it connects.
        let facts = match (answer, before) {
            (Err(_), Some(Ok(facts))) => Ok(facts),
            (answer, _) => answer,
        };
        {
            let mut kept = node.facts.kept.lock().expect("never poisoned");
            let machines = kept.entry(hub.clone()).or_default();
            machines.insert(name.clone(), Kept { since, facts });
            write(&node, &hub, machines);
        }
        node.facts.asking.lock().expect("never poisoned").remove(&(hub, name));
        node.shown_changed();
    });
}

fn read(node: &Node, hub: &HubId) -> BTreeMap<MachineName, Kept> {
    let path = crate::world::hub_dir(&node.world, hub).join(FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            node.log.line(format_args!("reading {}: {error}; the machines are asked again", path.display()));
            BTreeMap::new()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => {
            node.log.line(format_args!("reading {}: {error}; the machines are asked again", path.display()));
            BTreeMap::new()
        }
    }
}

fn write(node: &Node, hub: &HubId, machines: &BTreeMap<MachineName, Kept>) {
    let path = crate::world::hub_dir(&node.world, hub).join(FILE);
    let written = serde_json::to_vec(machines)
        .map_err(std::io::Error::other)
        .and_then(|bytes| fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private));
    if let Err(error) = written {
        node.log.line(format_args!("keeping what the machines said of themselves in {}: {error}", path.display()));
    }
}
