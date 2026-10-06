//! What a device asks of the whole World, it asks of each machine itself
//! (`world-network.md`, 机器与设备): each online machine its scope covers is
//! asked for its own part, answers only what that scope may see, and seals
//! its answer for this device alone; no machine reads another's answer on
//! the device's behalf. What only the projects' ledgers know, a project's
//! record or the World's projects, one machine answers from its own copy.

use std::sync::Arc;

use fastctx_net::link::{CallFailure, Link};
use fastctx_types::agents::{MachineAgents, WorldAgents};
use fastctx_types::project::TownSummary;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::since::{MachineSince, SinceEvent, SinceIssue, SinceKind, SinceTown, WorldSince};
use fastctx_types::town::{LookedUp, MachineWho, WhoScope, WorldWho};
use fastctx_types::world::{Node as NodeView, NodesReply, Revoked, Standing};
use fastctx_types::{Call, MachineName, ProjectCall, Reply, TownName};
use jiff::Timestamp;

use super::{Session, failed, link_state};
use crate::Error;

/// How one machine answered.
enum Heard {
    Answered(RemoteReply),
    Offline(Option<Timestamp>),
    NoAnswer(String),
}

impl Session {
    /// The machines this device's scope covers, by name.
    fn reachable(&self) -> Vec<MachineName> {
        let (table, _) = self.link.members();
        let mine = table.get(&self.link.membership().name).map(|me| me.scope.clone());
        table
            .machines()
            .filter(|member| mine.as_ref().is_none_or(|scope| scope.machines.contains(&member.name)))
            .map(|member| member.name.clone())
            .collect()
    }

    /// Asks each of `machines` at once, the online ones by a call `call`
    /// makes for each.
    async fn ask_each(
        &self,
        machines: Vec<MachineName>,
        call: impl Fn(&MachineName) -> RemoteCall,
    ) -> Vec<(MachineName, Heard)> {
        let refreshed = self.link.refresh().await;
        let (_, presence) = self.link.members();
        let mut heard = Vec::new();
        let mut asks = Vec::new();
        for name in machines {
            if let Err(error) = &refreshed {
                let reason = format!("whether it is connected is not known: the hub could not be asked ({error})");
                heard.push((name, Heard::NoAnswer(reason)));
                continue;
            }
            match presence.iter().find(|seen| seen.name == name) {
                Some(seen) if seen.online => {
                    let (link, call): (Arc<Link>, RemoteCall) = (self.link.clone(), call(&name));
                    asks.push(tokio::spawn(async move {
                        let answer = match link.call(&name, call).await {
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
                Err(error) => heard.push((self.link.membership().name.clone(), Heard::NoAnswer(error.to_string()))),
            }
        }
        heard.sort_by(|a, b| a.0.cmp(&b.0));
        heard
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
    /// machine; always only those this device's scope covers.
    fn asked(&self, scope: &WhoScope) -> Vec<MachineName> {
        let reachable = self.reachable();
        match scope {
            WhoScope::Town { record: Some(record), .. } => {
                reachable.into_iter().filter(|name| record.machines.contains(name)).collect()
            }
            _ => reachable,
        }
    }

    pub(super) async fn who(&self, all: bool, town: Option<TownName>) -> Result<WorldWho, Error> {
        let scope = match &town {
            Some(town) => self.town(town).await?,
            None => WhoScope::World { towns: self.towns().await? },
        };
        let heard = self.ask_each(self.asked(&scope), |_| RemoteCall::Who { all, town: town.clone() }).await;
        let machines = heard
            .into_iter()
            .map(|(machine, heard)| match heard {
                Heard::Answered(RemoteReply::Who(reply)) if reply.machine == machine => MachineWho::Answered { reply },
                Heard::Answered(_) => MachineWho::NoAnswer { machine, reason: "it answered something else".to_string() },
                Heard::Offline(since) => MachineWho::Offline { machine, since },
                Heard::NoAnswer(reason) => MachineWho::NoAnswer { machine, reason },
            })
            .collect();
        Ok(WorldWho { machine: self.link.membership().name.clone(), scope, machines })
    }

    pub(super) async fn since(&self, since: Timestamp, town: Option<TownName>) -> Result<WorldSince, Error> {
        let scope = match &town {
            Some(town) => self.town(town).await?,
            None => WhoScope::World { towns: Vec::new() },
        };
        let mut issues = Vec::new();
        let mut events = Vec::new();
        let mut undated = Vec::new();
        let (_, presence) = self.link.members();
        let asked = self.asked(&scope);
        for name in &asked {
            if let Some(seen) = presence.iter().find(|seen| seen.name == *name)
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
                Heard::Answered(_) => MachineSince::NoAnswer { machine, reason: "it answered something else".to_string() },
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
            machine: self.link.membership().name.clone(),
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
            let mut unasked = Vec::new();
            for (machine, heard) in heard {
                match heard {
                    Heard::Answered(RemoteReply::Looked { looked: Some(there) }) => found.push(there),
                    Heard::Answered(_) => {}
                    Heard::Offline(_) => unasked.push(format!("{machine} (offline)")),
                    Heard::NoAnswer(reason) => unasked.push(format!("{machine} ({reason})")),
                }
            }
            looked.push(LookedUp { id, found, unasked });
        }
        Ok(looked)
    }

    pub(super) async fn agents(&self, machine: Option<MachineName>) -> Result<WorldAgents, Error> {
        let reachable = self.reachable();
        let asked = match machine {
            Some(machine) if reachable.contains(&machine) => vec![machine],
            Some(machine) => return Err(failed(format!("{machine} is not a machine this device may use"))),
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
        Ok(WorldAgents { machine: self.link.membership().name.clone(), machines })
    }

    pub(super) async fn nodes(&self) -> Result<NodesReply, Error> {
        let membership = self.link.membership();
        let me = membership.name.clone();
        let heard = self.ask_each(self.reachable(), |_| RemoteCall::Facts).await;
        let (table, presence) = self.link.members();
        // Projects are shown by their ids here: a device keeps no ledger
        // that would name them.
        let standing = |member: &fastctx_net::trust::Member| Standing {
            scope: member.scope.named(|id| Some(id.to_string())).expect("every project has an id"),
            manage: member.manage,
            invited_by: member.invited_by.clone(),
            delegated: member.delegated,
            sealing: member.seal.is_some(),
        };
        let mut machines = vec![NodeView {
            name: me.clone(),
            online: Some(true),
            since: None,
            facts: Err("this device".to_string()),
            revoked: None,
            device: true,
            standing: table.get(&me).map(standing),
            place: fastctx_net::tree::place(&table, &me),
        }];
        for (name, heard) in heard {
            let seen = presence.iter().find(|seen| seen.name == name);
            let facts = match heard {
                Heard::Answered(RemoteReply::Facts(facts)) => Ok(facts),
                Heard::Answered(_) => Err("it answered something else".to_string()),
                Heard::Offline(_) => Err("it is offline".to_string()),
                Heard::NoAnswer(reason) => Err(reason),
            };
            machines.push(NodeView {
                standing: table.get(&name).map(standing),
                place: fastctx_net::tree::place(&table, &name),
                online: seen.map(|seen| seen.online),
                since: seen.and_then(|seen| seen.since),
                name,
                facts,
                revoked: None,
                device: false,
            });
        }
        for (name, revocation) in table.revoked_members() {
            machines.push(NodeView {
                name: name.clone(),
                online: None,
                since: None,
                facts: Err("revoked".to_string()),
                device: false,
                revoked: Some(Revoked { by: revocation.by.clone(), at: revocation.at, reason: revocation.reason.clone() }),
                standing: None,
                place: None,
            });
        }
        Ok(NodesReply {
            machine: me,
            world: Some(membership.world.to_string()),
            link: link_state(&self.link),
            machines,
            joined: Vec::new(),
        })
    }
}

/// The id a machine's node gives the same presence fact, so copies merge.
fn presence_id(machine: &MachineName, at: Timestamp, online: bool) -> String {
    let mut bytes = Vec::new();
    for part in [machine.as_str(), &at.to_string(), if online { "on" } else { "off" }] {
        bytes.extend_from_slice(part.as_bytes());
        bytes.push(0);
    }
    let digest: String = fastctx_net::sign::sha256(&bytes).iter().take(10).map(|byte| format!("{byte:02x}")).collect();
    format!("presence-{digest}")
}
