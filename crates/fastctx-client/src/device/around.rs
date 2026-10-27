//! What a device asks of the whole World, it asks of each machine itself
//! (`world-network.md`, 四种话): each online machine its accounts may use is
//! asked for its own part, for each of those accounts, and answers only what
//! that account may see; no machine answers for another. What only the
//! projects' ledgers know, a project's record or its projects, one machine
//! that acts for the device's account answers from its own copy.

use fastctx_types::access::Written;

use fastctx_net::around::{self, Heard, Reachable};
use fastctx_types::access::HubAccount;
use fastctx_types::agents::WorldAgents;
use fastctx_types::logins::WorldLogins;
use fastctx_types::project::TownSummary;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::since::{MachineSince, SinceEvent, SinceIssue, WorldSince};
use fastctx_types::town::{LookedUp, MachineWho, WhoScope, WorldWho};
use fastctx_types::world::{HubNodes, Node as NodeView, NodesReply};
use fastctx_types::{AccountName, Call, MachineName, ProjectCall, Reply, TownName};
use jiff::Timestamp;

use super::{Device, Session, failed};
use crate::Error;

impl Device {
    /// Each machine once for every account of this device that may use it;
    /// answers are merged by hub, machine and account.
    fn reachable_each(&self) -> Vec<Reachable> {
        let mut reachable = Vec::new();
        let sessions = self.sessions();
        let links: Vec<_> = sessions.iter().map(|session| session.link.clone()).collect();
        for session in sessions {
            // A hub this device cannot reach now leaves its machines
            // unanswered, never unlisted (`Link::last_view`).
            let Some(view) = session.link.last_view() else { continue };
            let logins: Vec<AccountName> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
            for machine in view.machines() {
                for account in logins.iter().filter(|account| {
                    view.admits(&view.me, account, &machine.name, fastctx_types::access::Purpose::Look).is_ok()
                }) {
                    reachable.push(Reachable {
                        link: session.link.clone(),
                        name: machine.name.clone(),
                        key: machine.key,
                        account: account.clone(),
                        written: fastctx_net::around::written_among(&links, &session.link, &machine.name),
                    });
                }
            }
        }
        reachable
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

    /// A project can have conversations and jobs on any reachable machine.
    fn asked(&self, _scope: &WhoScope) -> Vec<Reachable> {
        self.reachable_each()
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
        let heard = around::ask_each(self.asked(&scope), |_| RemoteCall::Who { all, town: town.clone(), since }).await;
        let machines =
            around::answers::collect(heard).into_iter().map(|answers| answers.who(since.is_some())).collect();
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
            |machine, source| {
                sessions
                    .iter()
                    .filter(|session| source.is_none_or(|source| session.link.membership().id == source.hub))
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
            if let Some(history) = &mut reply.history {
                for machine in &reply.machines {
                    if matches!(machine, MachineWho::NoAnswer { .. }) {
                        continue;
                    }
                    if let Some(observation) = machine.observation() {
                        history.events.extend(self.presence(&observation.source, since));
                    }
                }
                history.events =
                    fastctx_types::since::merge_events(std::mem::take(&mut history.events), &mut history.issues);
            }
        }
        Ok(reply)
    }

    fn presence(&self, source: &fastctx_types::observation::MachineSource, since: Timestamp) -> Option<SinceEvent> {
        let session = self.sessions().into_iter().find(|session| session.link.membership().id == source.hub)?;
        let view = session.link.view();
        let seen = view.member(&source.written.name)?;
        around::answers::presence(source, seen.online, seen.since, since)
    }

    pub(super) async fn since(&self, since: Timestamp, town: Option<TownName>) -> Result<WorldSince, Error> {
        let scope = match &town {
            Some(town) => self.town(town).await?,
            None => WhoScope::World { towns: Vec::new() },
        };
        let mut issues = Vec::new();
        let mut events = Vec::new();
        let mut undated = Vec::new();
        let heard = around::ask_each(self.asked(&scope), |_| RemoteCall::Since { since, town: town.clone() }).await;
        let mut machines = Vec::new();
        for group in around::answers::collect(heard) {
            let source = group.source.clone();
            let (status, read) = group.since();
            if !matches!(status, MachineSince::NoAnswer { .. }) {
                events.extend(self.presence(&source, since));
            }
            if let Some(mut reply) = read {
                events.append(&mut reply.events);
                fastctx_types::observation::union(&mut undated, reply.undated);
            }
            machines.push(status);
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
            let heard = around::ask_each(self.reachable_each(), |_| RemoteCall::Look { id: id.clone() }).await;
            let mut one = LookedUp { id, found: Vec::new(), jobs: Vec::new(), unasked: Vec::new() };
            for answers in around::answers::collect(heard) {
                answers.look(&mut one);
            }
            one.attach_jobs();
            looked.push(one);
        }
        Ok(looked)
    }

    pub(super) async fn agents(&self, machine: Option<Written>) -> Result<WorldAgents, Error> {
        let reachable = self.reachable_each();
        let asked: Vec<Reachable> = match machine {
            Some(machine) => {
                let session = self.reaching(&machine)?;
                let asked: Vec<Reachable> = reachable
                    .into_iter()
                    .filter(|one| {
                        one.link.membership().id == session.link.membership().id
                            && one.link.view().same(&one.name, &machine.name)
                    })
                    .collect();
                if asked.is_empty() {
                    return Err(failed(format!("{machine} is not a machine this device may use")));
                }
                asked
            }
            None => reachable,
        };
        let heard = around::ask_each(asked, |_| RemoteCall::Agents).await;
        let machines = around::answers::collect(heard).into_iter().map(|answers| answers.agents()).collect();
        Ok(WorldAgents { machine: self.here(), machines })
    }

    pub(super) async fn agent_logins(&self, machine: Option<Written>) -> Result<WorldLogins, Error> {
        let reachable = self.reachable_each();
        let asked: Vec<Reachable> = match machine {
            Some(machine) => {
                let session = self.reaching(&machine)?;
                let asked: Vec<Reachable> = reachable
                    .into_iter()
                    .filter(|one| {
                        one.link.membership().id == session.link.membership().id
                            && one.link.view().same(&one.name, &machine.name)
                    })
                    .collect();
                if asked.is_empty() {
                    return Err(failed(format!("{machine} is not a machine this device may use")));
                }
                asked
            }
            None => reachable,
        };
        let heard = around::ask_each(asked, |_| RemoteCall::AgentLogins).await;
        let machines = around::answers::collect(heard).into_iter().map(|answers| answers.logins()).collect();
        Ok(WorldLogins { machine: self.here(), machines })
    }

    pub(super) async fn nodes(&self) -> Result<NodesReply, Error> {
        let links = self.sessions().into_iter().map(|session| session.link.clone()).collect();
        let hubs = around::on_hubs(links, |link, fresh| async move {
            let view = link.view();
            let logins: Vec<AccountName> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
            // The machines its accounts may work on, as the view last had
            // them while the hub cannot be asked; one whose agents only
            // assist them is no machine of theirs to list.
            let listed = link.last_view().is_some();
            let reachable: Vec<Reachable> = view
                .machines()
                .filter(|_| listed)
                .filter_map(|machine| {
                    let account =
                        view.account_for(&logins, &view.me, &machine.name, fastctx_types::access::Purpose::Hands)?;
                    Some(Reachable {
                        link: link.clone(),
                        name: machine.name.clone(),
                        key: machine.key,
                        account,
                        written: (&machine.name).into(),
                    })
                })
                .collect();
            let online_known = fresh.is_ok();
            let heard = around::ask_refreshed(reachable, |_| RemoteCall::Facts, fresh).await;
            let machines = heard
                .into_iter()
                .map(|(asked, heard)| {
                    let name = asked.source.written.name;
                    let seen = view.machines().find(|machine| machine.key.to_string() == asked.source.key);
                    NodeView {
                        written: name.to_string(),
                        online: online_known.then(|| seen.map(|seen| seen.online)).flatten(),
                        since: seen.and_then(|seen| seen.since),
                        through: seen.and_then(|seen| seen.through.clone()),
                        allows_direct: seen.is_some_and(|seen| !seen.direct.is_empty()),
                        path: Some(link.path_to(&name)),
                        facts: match heard {
                            Heard::Answered(RemoteReply::Facts(facts)) => Ok(facts),
                            Heard::Answered(_) => Err("it answered something else".to_string()),
                            Heard::Offline(_) => Err("it is offline".to_string()),
                            // With the hub not asked, none of its machines
                            // was: what each said of itself when it last
                            // connected.
                            Heard::NoAnswer(reason) => match seen.filter(|_| !online_known) {
                                Some(seen) => self.kept_facts(&link, seen),
                                None => Err(reason),
                            },
                        },
                        owner: seen.and_then(|seen| seen.home.clone()),
                        group: seen.and_then(|seen| seen.group.clone()),
                        rights: logins.iter().map(|account| (account.clone(), view.rights(account, &name))).collect(),
                        name,
                    }
                })
                .collect();
            HubNodes {
                hub: link.membership().id.clone(),
                name: view.hub_name.clone(),
                link: link.state(),
                logins,
                home: None,
                here: Vec::new(),
                machines,
            }
        })
        .await;
        Ok(NodesReply { machine: self.here(), facts: None, hubs })
    }
}
