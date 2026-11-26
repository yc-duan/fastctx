//! What a device asks of the whole World, it asks of each machine itself
//! (`world-network.md`, 四种话): each online machine its accounts may use is
//! asked for its own part, for each of those accounts, and answers only what
//! that account may see; no machine answers for another. What only the
//! projects' ledgers know, a project's record or its projects, one machine
//! that acts for the device's account answers from its own copy.
//!
//! Machines are asked as their hubs last told who is connected, none of the
//! hubs asked again first: the views follow their hubs' changes on their own.
//! Each round is answered within its bound (`WAITED_ON`, `SEARCHED`); a
//! machine that does not answer by then is told as not answering. What a
//! machine says of its agents and logins, whole, is kept, and asked again
//! once its logins number moved, or who asks it or what they may do there
//! changed, or the device is not connected to its hub (`Kept`).

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

/// How long a question of every machine waits for each (`fastctx w`, the
/// conversations of late, agents and logins), as a computer's node waits.
const WAITED_ON: std::time::Duration = std::time::Duration::from_secs(5);
/// How long a search for one thing by its id waits for each machine.
const SEARCHED: std::time::Duration = std::time::Duration::from_secs(10);

/// What each machine last answered whole of one kind, by hub and machine
/// key: how it is written, what the answer stood on, and the answer.
pub(super) type Kept<T> = std::sync::Mutex<std::collections::HashMap<(String, String), (Written, Standing, T)>>;

/// What a machine's answer stands on: its logins number, and the accounts
/// of this device that ask it with what each may do there and the logins
/// there that assist each (with the share each holds and whether it manages
/// the login), as its hub tells them. Another account asking, or one that
/// may do or see less there, is answered anew: the machine answers each only
/// what that account may see (`logins::remote::seen`).
#[derive(Clone, PartialEq)]
pub(super) struct Standing {
    number: u64,
    askers: Vec<(AccountName, fastctx_types::access::Rights, Vec<fastctx_net::directory::Assist>)>,
}

/// What an answer of the machine of `asked`, one machine asked for each of
/// its accounts, stands on now, as its hub tells on the connection up now
/// (a view kept from before it may hold what was taken back meanwhile)
/// where it tells a logins number; none otherwise.
fn standing(asked: &[Reachable]) -> Option<Standing> {
    let first = asked.first()?;
    if !matches!(first.link.status(), fastctx_net::link::Status::Online { .. }) || !first.link.view_current() {
        return None;
    }
    let view = first.link.view();
    let member = view.machines().find(|member| member.key == first.key)?;
    let number = member.numbers?.logins?;
    let mut askers: Vec<_> = asked
        .iter()
        .map(|one| {
            let mut assists: Vec<_> = view.assisting(&one.account, &member.name).cloned().collect();
            assists.sort();
            (one.account.clone(), view.rights(&one.account, &member.name), assists)
        })
        .collect();
    askers.sort_by(|a, b| a.0.cmp(&b.0));
    askers.dedup();
    Some(Standing { number, askers })
}

/// `asked` by machine: each hub and machine key with the questions of it,
/// one for each account, and what an answer of it stands on now.
type ByMachine = Vec<((String, String), Vec<Reachable>, Option<Standing>)>;

fn by_machine(asked: Vec<Reachable>) -> ByMachine {
    let mut machines: ByMachine = Vec::new();
    for machine in asked {
        let at = (machine.link.membership().id.to_string(), machine.key.to_string());
        match machines.iter_mut().find(|(was, _, _)| *was == at) {
            Some((_, questions, _)) => questions.push(machine),
            None => machines.push((at, vec![machine], None)),
        }
    }
    for (_, questions, standing_now) in &mut machines {
        *standing_now = standing(questions);
    }
    machines
}

/// The answers of `kept` still standing for the machines of `asked`; and
/// the machines to ask, with what an answer of each will stand on.
fn kept_or_asked<T: Clone>(kept: &Kept<T>, asked: Vec<Reachable>) -> (Vec<(Written, T)>, ByMachine) {
    let kept = kept.lock().expect("never poisoned");
    let mut standing: Vec<(Written, T)> = Vec::new();
    let mut ask = Vec::new();
    for (at, questions, now) in by_machine(asked) {
        match (&now, kept.get(&at)) {
            (Some(now), Some((written, then, answer))) if then == now => {
                standing.push((written.clone(), answer.clone()))
            }
            _ => ask.push((at, questions, now)),
        }
    }
    (standing, ask)
}

/// Whether an answer was given for every account it was asked for.
fn whole(observation: Option<&fastctx_types::observation::Observation>) -> bool {
    observation.is_none_or(|observation| observation.unanswered.is_empty())
}

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
        let heard =
            around::ask_known(self.asked(&scope), |_| RemoteCall::Who { all, town: town.clone(), since }, WAITED_ON)
                .await;
        let machines =
            around::answers::collect(heard).into_iter().map(|answers| answers.who(since.is_some())).collect();
        let mut reply = WorldWho {
            machine: self.here(),
            scope,
            machines,
            you: None,
            history: None,
            others: Vec::new(),
            involved: Vec::new(),
        };
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
        let heard =
            around::ask_known(self.asked(&scope), |_| RemoteCall::Since { since, town: town.clone() }, WAITED_ON).await;
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
        // Every id at once, each answered in its place.
        Ok(futures_util::future::join_all(ids.into_iter().map(|id| async move {
            let heard =
                around::ask_known(self.reachable_each(), |_| RemoteCall::Look { id: id.clone() }, SEARCHED).await;
            let mut one = LookedUp { id, found: Vec::new(), jobs: Vec::new(), unasked: Vec::new() };
            for answers in around::answers::collect(heard) {
                answers.look(&mut one);
            }
            one.attach_jobs();
            one
        }))
        .await)
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
        let (mut machines, asked) = kept_or_asked(&self.agents_kept, asked);
        let mut standings = std::collections::HashMap::new();
        let mut questions = Vec::new();
        for (at, asked, standing) in asked {
            standings.insert(at, standing);
            questions.extend(asked);
        }
        let heard = around::ask_known(questions, |_| RemoteCall::Agents, WAITED_ON).await;
        for answers in around::answers::collect(heard) {
            let (at, written) =
                ((answers.source.hub.to_string(), answers.source.key.clone()), answers.source.written.clone());
            let answer = answers.agents();
            if let (Some(Some(standing)), fastctx_types::agents::MachineAgents::Answered { observation, .. }) =
                (standings.remove(&at), &answer)
                && whole(observation.as_ref())
            {
                self.agents_kept
                    .lock()
                    .expect("never poisoned")
                    .insert(at, (written.clone(), standing, answer.clone()));
            }
            machines.push((written, answer));
        }
        machines.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(WorldAgents { machine: self.here(), machines: machines.into_iter().map(|(_, answer)| answer).collect() })
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
        let (mut machines, asked) = kept_or_asked(&self.logins_kept, asked);
        let mut standings = std::collections::HashMap::new();
        let mut questions = Vec::new();
        for (at, asked, standing) in asked {
            standings.insert(at, standing);
            questions.extend(asked);
        }
        let heard = around::ask_known(questions, |_| RemoteCall::AgentLogins, WAITED_ON).await;
        for answers in around::answers::collect(heard) {
            let (at, written) =
                ((answers.source.hub.to_string(), answers.source.key.clone()), answers.source.written.clone());
            let answer = answers.logins();
            if let (Some(Some(standing)), fastctx_types::logins::MachineLoginsHeard::Answered { observation, .. }) =
                (standings.remove(&at), &answer)
                && whole(observation.as_ref())
            {
                self.logins_kept
                    .lock()
                    .expect("never poisoned")
                    .insert(at, (written.clone(), standing, answer.clone()));
            }
            machines.push((written, answer));
        }
        machines.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(WorldLogins { machine: self.here(), machines: machines.into_iter().map(|(_, answer)| answer).collect() })
    }

    pub(super) async fn nodes(&self) -> Result<NodesReply, Error> {
        let links: Vec<_> = self.sessions().into_iter().map(|session| session.link.clone()).collect();
        let workbench = super::world_now::workbench_shown(links.iter().map(|link| &**link));
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
                        strain: seen.and_then(|seen| seen.strain.filter(|_| online_known && seen.online)),
                        allows_direct: seen.is_some_and(|seen| !seen.direct.is_empty()),
                        path: Some(link.path_to(&name)),
                        facts: match heard {
                            Heard::Answered(RemoteReply::Facts(facts)) => Ok(facts),
                            Heard::Answered(_) => Err("it answered something else".to_string()),
                            Heard::Offline(_) => Err("it is offline".to_string()),
                            // With the hub not asked, none of its machines
                            // was: what each said of itself when it last
                            // connected.
                            Heard::NoAnswer(reason) | Heard::Late(reason) => match seen.filter(|_| !online_known) {
                                Some(seen) => self.kept_facts(&link, seen),
                                None => Err(reason),
                            },
                        },
                        owner: seen.and_then(|seen| seen.home.clone()),
                        group: seen.and_then(|seen| seen.group.clone()),
                        rights: logins.iter().map(|account| (account.clone(), view.rights(account, &name))).collect(),
                        numbers: seen.and_then(|seen| seen.numbers).filter(|_| online_known),
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
        Ok(NodesReply { machine: self.here(), facts: None, hubs, workbench })
    }
}
