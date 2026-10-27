//! The World as a device shows it, from what its links hold, asking no
//! machine for it (`world-network.md`, 近况): what the hubs told is afoot on
//! each machine its accounts may use, the machines as the views have them
//! with what each said of itself when it last connected, and the projects
//! as a machine last listed them, listed again only when a hub says they
//! changed or the device connects again.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fastctx_net::directory::Member;
use fastctx_net::ids::HubId;
use fastctx_net::link::{Elsewhere, Link, Status};
use fastctx_types::project::TownSummary;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::town::{MachineWho, WhoReply, WhoScope, WorldWho};
use fastctx_types::world::{HubNodes, MachineFacts, Node as NodeView, NodesReply, WorldNow};
use fastctx_types::{AccountName, Call, MachineName, ProjectCall, Reply};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Device, failed};
use crate::Error;

const FACTS: &str = "machine-facts.json";

/// What the device keeps to show the World without asking.
pub(super) struct Shown {
    /// The projects as a machine last listed them, and whether that is
    /// under way.
    towns: Mutex<Towns>,
    /// The projects are to be listed again: a hub said one changed, or the
    /// device connected again (`Handler`).
    pub(super) stale: Arc<AtomicBool>,
    facts: Mutex<HashMap<HubId, BTreeMap<MachineName, Kept>>>,
    asking: Mutex<HashSet<(HubId, MachineName)>>,
}

impl Default for Shown {
    fn default() -> Self {
        Self {
            towns: Mutex::default(),
            stale: Arc::new(AtomicBool::new(true)),
            facts: Mutex::default(),
            asking: Mutex::default(),
        }
    }
}

#[derive(Default)]
struct Towns {
    listed: Vec<TownSummary>,
    listing: bool,
}

/// What a machine said of itself, and when it had connected then.
#[derive(Clone, Serialize, Deserialize)]
struct Kept {
    since: Option<Timestamp>,
    facts: Result<MachineFacts, String>,
}

impl Device {
    /// Moves whenever what the device shows of the World may have changed.
    pub(crate) fn world_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.shown_changes.subscribe()
    }

    /// Whether what the hubs told of the machines is as each session on now
    /// first gave it (`fastctx_net::link::Link::afoot_ready`).
    pub(crate) fn afoot_ready(&self) -> bool {
        self.sessions().iter().all(|session| session.link.afoot_ready())
    }

    pub(crate) async fn world_now(self: &Arc<Self>) -> Result<WorldNow, Error> {
        let here = self.here();
        let towns = self.towns_listed();
        let who = WorldWho {
            machine: here.clone(),
            scope: WhoScope::World { towns: towns.clone() },
            you: None,
            machines: self.afoot(),
            history: None,
            others: Vec::new(),
        };
        let accounts = match Box::pin(self.call(Call::Accounts)).await? {
            (Reply::Accounts(accounts), _) => accounts,
            _ => return Err(failed("the device answered something other than its accounts")),
        };
        Ok(WorldNow { here, who, nodes: self.machines_now(), towns, accounts })
    }

    /// The projects as last listed, listed again in the background when a
    /// hub said they changed.
    fn towns_listed(self: &Arc<Self>) -> Vec<TownSummary> {
        let mut towns = self.shown.towns.lock().expect("never poisoned");
        let connected = self.sessions().iter().any(|session| matches!(session.link.status(), Status::Online { .. }));
        if !towns.listing && connected && self.shown.stale.swap(false, Ordering::AcqRel) {
            towns.listing = true;
            let device = self.clone();
            tokio::spawn(async move {
                let listed = match device.project(ProjectCall::List).await {
                    Ok(Reply::Towns { towns }) => Some(towns),
                    Ok(_) => None,
                    // Listed again when a hub next says they changed, or the
                    // device connects again.
                    Err(_) => None,
                };
                {
                    let mut towns = device.shown.towns.lock().expect("never poisoned");
                    towns.listing = false;
                    if let Some(listed) = listed {
                        towns.listed = listed;
                    }
                }
                super::changed(&device.shown_changes);
            });
        }
        towns.listed.clone()
    }

    /// Each machine the device's accounts may use, as its hub last told
    /// what is afoot on it.
    fn afoot(&self) -> Vec<MachineWho> {
        let mut machines = Vec::new();
        for session in self.sessions() {
            // While the line to a hub is down, its machines are as it last
            // told of them.
            let Some(view) = session.link.last_view() else { continue };
            let current = matches!(session.link.status(), Status::Online { .. });
            let told = session.link.afoot();
            let logins: Vec<AccountName> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
            for member in view.machines() {
                if !logins.iter().any(|account| view.call_rights(&view.me, account, &member.name).any()) {
                    continue;
                }
                let name = member.name.clone();
                let afoot = told.get(&name);
                let reply = |afoot: &Elsewhere| WhoReply {
                    machine: name.clone(),
                    residents: afoot.seen.residents.values().cloned().collect(),
                    jobs: afoot.seen.jobs.values().cloned().collect(),
                    history: None,
                    busy: afoot.busy,
                };
                machines.push(match (member.online, afoot) {
                    (false, afoot) => MachineWho::Offline {
                        observation: None,
                        machine: name.clone(),
                        since: member.since,
                        last: afoot.map(|afoot| Box::new(reply(afoot))),
                    },
                    (true, Some(afoot)) => MachineWho::Answered { observation: None, reply: reply(afoot) },
                    (true, None) if !current => MachineWho::NoAnswer {
                        observation: None,
                        machine: name.clone(),
                        reason: "this device is not connected to the hub now".into(),
                    },
                    (true, None) if !view.afoot => MachineWho::NoAnswer {
                        observation: None,
                        machine: name.clone(),
                        reason: "the hub runs an older Fastctx, which does not pass on what is on each machine".into(),
                    },
                    (true, None) => MachineWho::NoAnswer {
                        observation: None,
                        machine: name.clone(),
                        reason: "it runs an older Fastctx, which does not tell what is on it".into(),
                    },
                });
            }
        }
        machines.sort_by(|a, b| machine_of(a).cmp(machine_of(b)));
        machines
    }

    /// The machines the device's accounts may work on, hub by hub, as the
    /// views have them, with what each said of itself when it last
    /// connected.
    fn machines_now(self: &Arc<Self>) -> NodesReply {
        let mut hubs = Vec::new();
        for session in self.sessions() {
            let view = session.link.view();
            // A view kept while the line to the hub is down still has the
            // machines, and says who was online then, not now.
            let current = matches!(session.link.status(), Status::Online { .. });
            let listed = session.link.last_view().is_some();
            let logins: Vec<AccountName> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
            let mut machines = Vec::new();
            for member in view.machines().filter(|_| listed) {
                let Some(account) =
                    logins.iter().find(|account| view.call_rights(&view.me, account, &member.name).hands)
                else {
                    continue;
                };
                let facts = self.facts_of(&session.link, member, current.then(|| account.clone()));
                machines.push(NodeView {
                    name: member.name.clone(),
                    written: member.name.to_string(),
                    online: current.then_some(member.online),
                    since: member.since,
                    through: member.through.clone(),
                    allows_direct: !member.direct.is_empty(),
                    path: Some(session.link.path_to(&member.name)),
                    facts,
                    owner: member.home.clone(),
                    group: member.group.clone(),
                    rights: logins
                        .iter()
                        .map(|account| (account.clone(), view.call_rights(&view.me, account, &member.name)))
                        .collect(),
                });
            }
            hubs.push(HubNodes {
                hub: session.link.membership().id.clone(),
                name: view.hub_name.clone(),
                link: session.link.state(),
                logins,
                home: None,
                here: Vec::new(),
                machines,
            });
        }
        NodesReply { machine: self.here(), facts: None, hubs }
    }

    /// What `member` said of itself, as kept; asked again for `account` in
    /// the background when it is online and has connected since.
    fn facts_of(
        self: &Arc<Self>,
        link: &Arc<Link>,
        member: &Member,
        account: Option<AccountName>,
    ) -> Result<MachineFacts, String> {
        let hub = link.membership().id.clone();
        let kept = self.kept(&hub, &member.name);
        let current = kept.as_ref().is_some_and(|kept| kept.since == member.since);
        if member.online
            && !current
            && let Some(account) = account
            && self.shown.asking.lock().expect("never poisoned").insert((hub.clone(), member.name.clone()))
        {
            let before = kept.as_ref().map(|kept| kept.facts.clone());
            let (device, link, name, since) = (self.clone(), link.clone(), member.name.clone(), member.since);
            tokio::spawn(async move {
                let purpose = fastctx_types::access::Purpose::Look;
                let answer = match link.call(&name, &account, purpose, RemoteCall::Facts).await {
                    Ok(Ok(RemoteReply::Facts(facts))) => Ok(facts),
                    Ok(Ok(_)) => Err("it answered something else".to_string()),
                    Ok(Err(failure)) => Err(failure.message),
                    Err(failure) => Err(failure.to_string()),
                };
                // What it said before stands when it could not say it again.
                let facts = match (answer, before) {
                    (Err(_), Some(Ok(facts))) => Ok(facts),
                    (answer, _) => answer,
                };
                {
                    let mut kept = device.shown.facts.lock().expect("never poisoned");
                    let machines = kept.entry(hub.clone()).or_default();
                    machines.insert(name.clone(), Kept { since, facts });
                    device.write_facts(&hub, machines);
                }
                device.shown.asking.lock().expect("never poisoned").remove(&(hub, name));
                super::changed(&device.shown_changes);
            });
        }
        said(kept, member)
    }

    /// What `member` said of itself when it last connected, as kept, asking
    /// nothing.
    pub(super) fn kept_facts(&self, link: &Link, member: &Member) -> Result<MachineFacts, String> {
        said(self.kept(&link.membership().id, &member.name), member)
    }

    fn kept(&self, hub: &HubId, name: &MachineName) -> Option<Kept> {
        let mut facts = self.shown.facts.lock().expect("never poisoned");
        let machines = facts.entry(hub.clone()).or_insert_with(|| self.read_facts(hub));
        machines.get(name).cloned()
    }

    fn read_facts(&self, hub: &HubId) -> BTreeMap<MachineName, Kept> {
        let path = self.store.net(hub).path().join(FACTS);
        std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    fn write_facts(&self, hub: &HubId, machines: &BTreeMap<MachineName, Kept>) {
        let path = self.store.net(hub).path().join(FACTS);
        let written = serde_json::to_vec(machines)
            .map_err(std::io::Error::other)
            .and_then(|bytes| fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private));
        if let Err(error) = written {
            eprintln!("World device: keeping what the machines said of themselves: {error}");
        }
    }
}

fn machine_of(who: &MachineWho) -> &MachineName {
    match who {
        MachineWho::Answered { reply, .. } => &reply.machine,
        MachineWho::Offline { machine, .. } | MachineWho::NoAnswer { machine, .. } => machine,
    }
}

/// What a machine said of itself as kept, or why there is nothing kept.
fn said(kept: Option<Kept>, member: &Member) -> Result<MachineFacts, String> {
    match kept {
        Some(kept) => kept.facts,
        None if member.online => Err("it has not said yet".to_string()),
        None => Err("it is offline".to_string()),
    }
}
