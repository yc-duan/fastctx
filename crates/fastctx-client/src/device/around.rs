//! What a device asks of the whole World, it asks of each machine itself
//! (`world-network.md`, 四种话): each online machine its accounts may use is
//! asked for its own part, for one of those accounts, and answers only what
//! that account may see; no machine answers for another. What only the
//! projects' ledgers know, a project's record or its projects, one machine
//! that acts for the device's account answers from its own copy.

use std::sync::Arc;

use fastctx_net::link::{CallFailure, Link};
use fastctx_types::access::HubAccount;
use fastctx_types::agents::{MachineAgents, WorldAgents};
use fastctx_types::logins::{MachineLoginsHeard, WorldLogins};
use fastctx_types::project::TownSummary;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::since::{MachineSince, SinceEvent, SinceIssue, SinceKind, SinceTown, WorldSince};
use fastctx_types::town::{LookedUp, MachineWho, WhoScope, WorldWho};
use fastctx_types::world::{HubNodes, Node as NodeView, NodesReply};
use fastctx_types::{AccountName, Call, MachineName, ProjectCall, Reply, TownName};
use jiff::Timestamp;

use super::{Device, Session, failed};
use crate::Error;

/// How one machine answered.
enum Heard {
    Answered(RemoteReply),
    Offline(Option<Timestamp>),
    NoAnswer(String),
}

/// A machine to ask, on the hub `link` reaches it through, with the
/// device's account that may use it.
struct Reachable {
    link: Arc<Link>,
    name: MachineName,
    account: AccountName,
}

impl Device {
    /// The machines this device's accounts may use, on every hub it is on.
    fn reachable(&self) -> Vec<Reachable> {
        let mut reachable = Vec::new();
        for session in self.sessions() {
            let view = session.link.view();
            let logins: Vec<AccountName> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
            for machine in view.machines() {
                if let Some(account) = logins.iter().find(|account| view.rights(account, &machine.name).any()) {
                    reachable.push(Reachable {
                        link: session.link.clone(),
                        name: machine.name.clone(),
                        account: account.clone(),
                    });
                }
            }
        }
        reachable
    }

    /// Asks each of `machines` at once, the online ones by a call `call`
    /// makes for each.
    async fn ask_each(
        &self,
        machines: Vec<Reachable>,
        call: impl Fn(&MachineName) -> RemoteCall,
    ) -> Vec<(MachineName, Heard)> {
        let mut heard = Vec::new();
        let mut asks = Vec::new();
        for Reachable { link, name, account } in machines {
            if let Err(error) = link.refresh().await {
                let reason = format!("whether it is connected is not known: the hub could not be asked ({error})");
                heard.push((name, Heard::NoAnswer(reason)));
                continue;
            }
            let seen = link.view().member(&name).cloned();
            match seen {
                Some(seen) if seen.online => {
                    let call = call(&name);
                    asks.push(tokio::spawn(async move {
                        let purpose = call.purpose();
                        let answer = match link.call(&name, &account, purpose, call).await {
                            Ok(Ok(reply)) => Heard::Answered(reply),
                            Ok(Err(failure)) => Heard::NoAnswer(failure.message),
                            Err(CallFailure::Offline { last_seen, .. }) => Heard::Offline(last_seen),
                            Err(failure) => Heard::NoAnswer(failure.to_string()),
                        };
                        (name, answer)
                    }));
                }
                Some(seen) => heard.push((name, Heard::Offline(seen.since))),
                None => heard.push((name, Heard::NoAnswer("the hub did not say whether it is connected".to_string()))),
            }
        }
        for ask in asks {
            match ask.await {
                Ok(answer) => heard.push(answer),
                Err(error) => heard.push((self.here(), Heard::NoAnswer(error.to_string()))),
            }
        }
        heard.sort_by(|a, b| a.0.cmp(&b.0));
        heard
    }

    pub(super) fn here(&self) -> MachineName {
        self.state().name.unwrap_or_else(|| MachineName::try_from("device").expect("a valid name"))
    }

    /// A project's record and the machines it names, from a machine's copy.
    async fn town(&self, town: &TownName) -> Result<WhoScope, Error> {
        match self.relay(Call::Project(ProjectCall::Show { town: Some(town.clone()) })).await {
            Ok((Reply::Record { town, record, .. }, _)) => Ok(WhoScope::Town { town, record: Some(record) }),
            Ok(_) => Err(failed("the machine asked answered something other than the project's record")),
            Err(_) => Ok(WhoScope::Town { town: town.clone(), record: None }),
        }
    }

    async fn towns(&self) -> Result<Vec<TownSummary>, Error> {
        match self.relay(Call::Project(ProjectCall::List)).await? {
            (Reply::Towns { towns }, _) => Ok(towns),
            _ => Err(failed("the machine asked answered something other than the projects")),
        }
    }

    /// The machines to ask about `scope`: those its project names, or every
    /// machine; always only those this device's accounts may use.
    fn asked(&self, scope: &WhoScope) -> Vec<Reachable> {
        let reachable = self.reachable();
        match scope {
            WhoScope::Town { record: Some(record), .. } => {
                reachable.into_iter().filter(|machine| record.machines.contains(&machine.name)).collect()
            }
            _ => reachable,
        }
    }

    pub(super) async fn who(
        &self,
        all: bool,
        town: Option<TownName>,
        since: Option<Timestamp>,
    ) -> Result<WorldWho, Error> {
        let scope = match &town {
            Some(town) => self.town(town).await?,
            None => WhoScope::World { towns: self.towns().await? },
        };
        let heard = self.ask_each(self.asked(&scope), |_| RemoteCall::Who { all, town: town.clone(), since }).await;
        let machines = heard
            .into_iter()
            .map(|(machine, heard)| match heard {
                Heard::Answered(RemoteReply::Who(reply)) if reply.machine == machine => MachineWho::Answered { reply },
                Heard::Answered(_) => {
                    MachineWho::NoAnswer { machine, reason: "it answered something else".to_string() }
                }
                Heard::Offline(since) => MachineWho::Offline { machine, since, last: None },
                Heard::NoAnswer(reason) => MachineWho::NoAnswer { machine, reason },
            })
            .collect();
        let mut reply =
            WorldWho { machine: self.here(), scope, machines, you: None, history: None, others: Vec::new() };
        // This device works for the accounts it is logged in to, on each hub.
        let sessions = self.sessions();
        let logins = |session: &Session| -> Vec<AccountName> {
            session.link.view().me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default()
        };
        reply.mark_others(
            |account| {
                sessions.iter().any(|session| {
                    session.link.membership().id == account.hub && logins(session).contains(&account.account)
                })
            },
            |machine| {
                sessions
                    .iter()
                    .flat_map(|session| {
                        let hub = session.link.membership().id.clone();
                        let logins = session.link.view().member(machine).map(|member| member.logins.clone());
                        logins.into_iter().flatten().map(move |account| HubAccount { hub: hub.clone(), account })
                    })
                    .collect()
            },
        );
        if let Some(since) = since {
            reply.collect_history(since, town);
        }
        Ok(reply)
    }

    pub(super) async fn since(&self, since: Timestamp, town: Option<TownName>) -> Result<WorldSince, Error> {
        let scope = match &town {
            Some(town) => self.town(town).await?,
            None => WhoScope::World { towns: Vec::new() },
        };
        let mut issues = Vec::new();
        let mut events = Vec::new();
        let mut undated = Vec::new();
        let asked = self.asked(&scope);
        for machine in &asked {
            let name = &machine.name;
            if let Some(seen) = machine.link.view().member(name).cloned()
                && let Some(at) = seen.since.filter(|at| *at > since)
            {
                events.push(SinceEvent {
                    id: presence_id(name, at, seen.online),
                    at,
                    machine: name.clone(),
                    town: SinceTown::Unrecorded { recorded_in: Vec::new() },
                    sources: Vec::new(),
                    what: if seen.online { SinceKind::MachineConnected } else { SinceKind::MachineOffline },
                });
            }
        }
        let heard = self.ask_each(asked, |_| RemoteCall::Since { since, town: town.clone() }).await;
        let mut machines = Vec::new();
        for (machine, heard) in heard {
            machines.push(match heard {
                Heard::Answered(RemoteReply::Since(mut reply)) if reply.coverage.machine == machine => {
                    events.append(&mut reply.events);
                    undated.append(&mut reply.undated);
                    MachineSince::Answered { reply: reply.coverage }
                }
                Heard::Answered(_) => {
                    MachineSince::NoAnswer { machine, reason: "it answered something else".to_string() }
                }
                Heard::Offline(since) => MachineSince::Offline { machine, since },
                Heard::NoAnswer(reason) => MachineSince::NoAnswer { machine, reason },
            });
        }
        if matches!(&scope, WhoScope::Town { record: None, .. }) {
            issues.push(SinceIssue {
                source: "World ledgers".to_string(),
                reason: "no machine answered with the project's record, so every machine was asked".to_string(),
            });
        }
        let events = fastctx_types::since::merge_events(events, &mut issues);
        Ok(WorldSince {
            machine: self.here(),
            since,
            town,
            machines,
            events,
            undated,
            issues,
            limitations: fastctx_types::since::limitations(),
        })
    }

    pub(super) async fn look(&self, ids: Vec<String>) -> Result<Vec<LookedUp>, Error> {
        let mut looked = Vec::new();
        for id in ids {
            let heard = self.ask_each(self.reachable(), |_| RemoteCall::Look { id: id.clone() }).await;
            let mut found = Vec::new();
            let mut jobs = Vec::new();
            let mut unasked = Vec::new();
            for (machine, heard) in heard {
                match heard {
                    Heard::Answered(RemoteReply::Looked { looked, jobs: work }) => {
                        found.extend(looked);
                        jobs.extend(work);
                    }
                    Heard::Answered(_) => {}
                    Heard::Offline(_) => unasked.push(format!("{machine} (offline)")),
                    Heard::NoAnswer(reason) => unasked.push(format!("{machine} ({reason})")),
                }
            }
            let mut one = LookedUp { id, found, unasked, jobs };
            one.attach_jobs();
            looked.push(one);
        }
        Ok(looked)
    }

    pub(super) async fn agents(&self, machine: Option<MachineName>) -> Result<WorldAgents, Error> {
        let reachable = self.reachable();
        let asked: Vec<Reachable> = match machine {
            Some(machine) => {
                let asked: Vec<Reachable> = reachable.into_iter().filter(|one| one.name == machine).collect();
                if asked.is_empty() {
                    return Err(failed(format!("{machine} is not a machine this device may use")));
                }
                asked
            }
            None => reachable,
        };
        let machines = self
            .ask_each(asked, |_| RemoteCall::Agents)
            .await
            .into_iter()
            .map(|(machine, heard)| match heard {
                Heard::Answered(RemoteReply::Agents(reply)) if reply.machine == machine => {
                    MachineAgents::Answered { reply }
                }
                Heard::Answered(_) => MachineAgents::NoAnswer { machine, reason: "it answered something else".into() },
                Heard::Offline(since) => MachineAgents::Offline { machine, since },
                Heard::NoAnswer(reason) => MachineAgents::NoAnswer { machine, reason },
            })
            .collect();
        Ok(WorldAgents { machine: self.here(), machines })
    }

    pub(super) async fn agent_logins(&self, machine: Option<MachineName>) -> Result<WorldLogins, Error> {
        let reachable = self.reachable();
        let asked: Vec<Reachable> = match machine {
            Some(machine) => {
                let asked: Vec<Reachable> = reachable.into_iter().filter(|one| one.name == machine).collect();
                if asked.is_empty() {
                    return Err(failed(format!("{machine} is not a machine this device may use")));
                }
                asked
            }
            None => reachable,
        };
        let machines = self
            .ask_each(asked, |_| RemoteCall::AgentLogins)
            .await
            .into_iter()
            .map(|(machine, heard)| match heard {
                Heard::Answered(RemoteReply::AgentLogins(logins)) if logins.machine == machine => {
                    MachineLoginsHeard::Answered { logins }
                }
                Heard::Answered(_) => MachineLoginsHeard::NoAnswer {
                    machine,
                    reason: "it answered something else; the World there may be older".into(),
                },
                Heard::Offline(since) => MachineLoginsHeard::Offline { machine, since },
                Heard::NoAnswer(reason) => MachineLoginsHeard::NoAnswer { machine, reason },
            })
            .collect();
        Ok(WorldLogins { machine: self.here(), machines })
    }

    pub(super) async fn nodes(&self) -> Result<NodesReply, Error> {
        let mut hubs = Vec::new();
        for session in self.sessions() {
            let view = session.link.view();
            let logins: Vec<AccountName> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
            // The machines its accounts may work on; one whose agents only
            // assist them is no machine of theirs to list.
            let reachable: Vec<Reachable> = self
                .reachable()
                .into_iter()
                .filter(|one| Arc::ptr_eq(&one.link, &session.link))
                .filter(|one| logins.iter().any(|account| view.rights(account, &one.name).hands))
                .collect();
            let heard = self.ask_each(reachable, |_| RemoteCall::Facts).await;
            let machines = heard
                .into_iter()
                .map(|(name, heard)| {
                    let seen = view.member(&name);
                    NodeView {
                        written: name.to_string(),
                        online: seen.map(|seen| seen.online),
                        since: seen.and_then(|seen| seen.since),
                        through: seen.and_then(|seen| seen.through.clone()),
                        allows_direct: seen.is_some_and(|seen| !seen.direct.is_empty()),
                        path: Some(session.link.path_to(&name)),
                        facts: match heard {
                            Heard::Answered(RemoteReply::Facts(facts)) => Ok(facts),
                            Heard::Answered(_) => Err("it answered something else".to_string()),
                            Heard::Offline(_) => Err("it is offline".to_string()),
                            Heard::NoAnswer(reason) => Err(reason),
                        },
                        owner: seen.and_then(|seen| seen.home.clone()),
                        group: seen.and_then(|seen| seen.group.clone()),
                        rights: logins.iter().map(|account| (account.clone(), view.rights(account, &name))).collect(),
                        name,
                    }
                })
                .collect();
            hubs.push(HubNodes {
                hub: session.link.membership().id.clone(),
                name: view.hub_name.clone(),
                link: session.link.state(),
                logins,
                home: None,
                machines,
            });
        }
        Ok(NodesReply { machine: self.here(), facts: None, hubs })
    }
}

/// The id a machine's node gives the same presence fact, so copies merge.
fn presence_id(machine: &MachineName, at: Timestamp, online: bool) -> String {
    let mut bytes = Vec::new();
    for part in [machine.key(), &at.to_string(), if online { "on" } else { "off" }] {
        bytes.extend_from_slice(part.as_bytes());
        bytes.push(0);
    }
    let digest: String = fastctx_net::sign::sha256(&bytes).iter().take(10).map(|byte| format!("{byte:02x}")).collect();
    format!("presence-{digest}")
}
