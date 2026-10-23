//! What `fastctx w` reads: who is here, and what each is doing
//! (`world-town.md`).

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::project::{Record, TownSummary};
use crate::residents::{About, Action, MessageLine, Moved, Now};
use crate::run::JobRecord;
use crate::{JobId, MachineName, ResidentId, TownName};

/// One machine's answer to `fastctx w`: the residents that live on it, and
/// every job still running on it, whoever started it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoReply {
    pub machine: MachineName,
    pub residents: Vec<ResidentNow>,
    pub jobs: Vec<RunningJob>,
    /// The same local history reader used by `Since`, included in this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<crate::since::SinceReply>,
    /// How many agents are at work or waiting for someone on the machine,
    /// whosever they are: told its owner alone (`world-gui.md`, 机器页).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<u32>,
}

impl WhoReply {
    /// The same machine's answer to another of the accounts whoever asks
    /// works for, merged into this one: a machine shows each account the
    /// conversations working for it alone.
    pub fn merge(&mut self, more: WhoReply) {
        for resident in more.residents {
            if let Some(known) = self.residents.iter_mut().find(|known| known.about.id == resident.about.id) {
                *known = resident;
            } else {
                self.residents.push(resident);
            }
        }
        for job in more.jobs {
            if let Some(known) = self.jobs.iter_mut().find(|known| known.job == job.job) {
                *known = job;
            } else {
                self.jobs.push(job);
            }
        }
        match (&mut self.history, more.history) {
            (Some(history), Some(more)) => {
                history.merge(more);
            }
            (history @ None, more) => *history = more,
            (Some(_), None) => {}
        }
        self.busy = self.busy.or(more.busy);
    }
}

/// What is afoot on a machine (`world-network.md`, 近况): its residents and
/// the jobs running on it, by id. The machine keeps its own whole, its hub
/// the latest each machine told, and each member what it was told of each
/// machine; between them goes only what changed (`AfootChange`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AfootState<R, J> {
    pub residents: BTreeMap<ResidentId, R>,
    pub jobs: BTreeMap<JobId, J>,
}

impl<R, J> Default for AfootState<R, J> {
    fn default() -> Self {
        Self { residents: BTreeMap::new(), jobs: BTreeMap::new() }
    }
}

/// How what is afoot on a machine changed: what came or changed, whole, and
/// what went, by id. `anew` starts from nothing, as when the one telling
/// cannot know what the one told still holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AfootChange<R, J> {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub anew: bool,
    #[serde(default = "Vec::new", skip_serializing_if = "Vec::is_empty")]
    pub residents: Vec<R>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub residents_gone: Vec<ResidentId>,
    #[serde(default = "Vec::new", skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<J>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs_gone: Vec<JobId>,
}

impl<R, J> Default for AfootChange<R, J> {
    fn default() -> Self {
        Self { anew: false, residents: Vec::new(), residents_gone: Vec::new(), jobs: Vec::new(), jobs_gone: Vec::new() }
    }
}

impl<R, J> AfootChange<R, J> {
    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        !self.anew
            && self.residents.is_empty()
            && self.residents_gone.is_empty()
            && self.jobs.is_empty()
            && self.jobs_gone.is_empty()
    }
}

impl<R: Afloat<Id = ResidentId>, J: Afloat<Id = JobId>> AfootChange<R, J> {
    /// This change and `later` as one: applying it does what applying this
    /// and then `later` does. A member told late is told once.
    pub fn then(mut self, later: Self) -> Self {
        if later.anew {
            return later;
        }
        for id in later.residents_gone {
            self.residents.retain(|resident| resident.id() != id);
            if !self.residents_gone.contains(&id) {
                self.residents_gone.push(id);
            }
        }
        for resident in later.residents {
            let id = resident.id();
            self.residents.retain(|kept| kept.id() != id);
            self.residents_gone.retain(|gone| *gone != id);
            self.residents.push(resident);
        }
        for id in later.jobs_gone {
            self.jobs.retain(|job| job.id() != id);
            if !self.jobs_gone.contains(&id) {
                self.jobs_gone.push(id);
            }
        }
        for job in later.jobs {
            let id = job.id();
            self.jobs.retain(|kept| kept.id() != id);
            self.jobs_gone.retain(|gone| *gone != id);
            self.jobs.push(job);
        }
        self
    }
}

/// Something afoot, known by its id.
pub trait Afloat {
    type Id: Ord + Clone;
    fn id(&self) -> Self::Id;
}

impl Afloat for ResidentNow {
    type Id = ResidentId;
    fn id(&self) -> ResidentId {
        self.about.id.clone()
    }
}

impl Afloat for RunningJob {
    type Id = JobId;
    fn id(&self) -> JobId {
        self.job.clone()
    }
}

impl Afloat for AfootResident {
    type Id = ResidentId;
    fn id(&self) -> ResidentId {
        self.resident.about.id.clone()
    }
}

impl Afloat for AfootJob {
    type Id = JobId;
    fn id(&self) -> JobId {
        self.job.job.clone()
    }
}

impl<R, J> AfootState<R, J>
where
    R: Afloat<Id = ResidentId> + Clone + PartialEq,
    J: Afloat<Id = JobId> + Clone + PartialEq,
{
    /// Takes in what changed.
    pub fn apply(&mut self, change: AfootChange<R, J>) {
        if change.anew {
            self.residents.clear();
            self.jobs.clear();
        }
        for id in change.residents_gone {
            self.residents.remove(&id);
        }
        for resident in change.residents {
            self.residents.insert(resident.id(), resident);
        }
        for id in change.jobs_gone {
            self.jobs.remove(&id);
        }
        for job in change.jobs {
            self.jobs.insert(job.id(), job);
        }
    }

    /// What changed from this to `now`.
    pub fn change_to(&self, now: &Self) -> AfootChange<R, J> {
        AfootChange {
            anew: false,
            residents: now
                .residents
                .iter()
                .filter(|(id, resident)| self.residents.get(id) != Some(resident))
                .map(|(_, resident)| resident.clone())
                .collect(),
            residents_gone: self.residents.keys().filter(|id| !now.residents.contains_key(id)).cloned().collect(),
            jobs: now
                .jobs
                .iter()
                .filter(|(id, job)| self.jobs.get(id) != Some(job))
                .map(|(_, job)| job.clone())
                .collect(),
            jobs_gone: self.jobs.keys().filter(|id| !now.jobs.contains_key(id)).cloned().collect(),
        }
    }

    /// All of it, from nothing.
    pub fn whole(&self) -> AfootChange<R, J> {
        AfootChange { anew: true, ..Self::default().change_to(self) }
    }
}

/// What is afoot on a machine as the machine tells its hub.
pub type Afoot = AfootState<AfootResident, AfootJob>;
/// What is afoot on a machine as a member is told of it.
pub type AfootSeen = AfootState<ResidentNow, RunningJob>;

/// A resident as its machine tells it, at the grain of turns, with the
/// accounts of that hub it works for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AfootResident {
    pub resident: ResidentNow,
    pub accounts: Vec<crate::AccountName>,
}

/// A job running on a machine as the machine tells it, with the accounts of
/// that hub it runs for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AfootJob {
    pub job: RunningJob,
    pub accounts: Vec<crate::AccountName>,
}

impl ResidentNow {
    /// At work, or waiting for someone to answer it.
    pub fn in_progress(&self) -> bool {
        self.now.as_ref().is_some_and(|now| {
            now.activity == crate::residents::Activity::Busy || now.waiting_for_user || now.asked.is_some()
        })
    }

    /// As it is told while it changes only from turn to turn: without the
    /// tool it is in and the last thing it did.
    pub fn at_turn_grain(mut self) -> Self {
        self.tools.clear();
        self.last_action = None;
        if let Some(now) = &mut self.now {
            now.last_tool_at = None;
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResidentNow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::observation::MachineSource>,
    pub about: About,
    pub town: Option<TownName>,
    /// Whether it is here: a session of it is connected, or, for a
    /// session the World holds, it is not asleep.
    pub here: bool,
    /// What its host reports it doing, when its host tells the World.
    pub now: Option<Now>,
    pub last_action: Option<Action>,
    /// Current host tool calls, including tools outside the World.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<CurrentTool>,
    /// It lived in `town` and moved away: what it did there stays there.
    #[serde(default)]
    pub moved: Option<Moved>,
    /// Removed from the World, with records preserved in its archive.
    #[serde(default)]
    pub removed: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentTool {
    pub turn: String,
    pub id: String,
    pub started_at: Option<Timestamp>,
    pub name: String,
    pub machine: Option<MachineName>,
    /// The command first line, affected paths, or a named host argument.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningJob {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::observation::MachineSource>,
    pub job: JobId,
    #[serde(default)]
    pub town: Option<TownName>,
    /// Where it runs.
    pub machine: MachineName,
    pub owner: crate::run::JobOwner,
    pub started_at: Timestamp,
    pub pid: Option<u32>,
    /// The script's first line.
    pub command: String,
    /// The log holding its output, on that machine.
    pub output: String,
    /// Separate standard error, when the job keeps it apart from stdout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// The account it runs for, when another machine sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<crate::access::HubAccount>,
}

/// `fastctx w` in a town or across the World: each machine's answer, or why
/// there is none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldWho {
    /// The machine that asked.
    pub machine: MachineName,
    pub scope: WhoScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub you: Option<crate::Address>,
    pub machines: Vec<MachineWho>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<crate::since::WorldSince>,
    /// The residents and jobs shown that work for none of the asker's
    /// accounts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub others: Vec<WhoFor>,
}

/// A resident or a job on `machine`, by its id, that works for other
/// accounts than the asker's: a machine shared with several accounts shows
/// them all to those that may work on it, each marked with whom it works
/// for (`world-network.md`, 共享机器与借出智能体).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoFor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::observation::MachineSource>,
    pub machine: MachineName,
    pub id: String,
    pub accounts: Vec<crate::AccountName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WhoScope {
    /// One town: the machines its record names were asked, or every machine
    /// when this one keeps no record of it.
    Town { town: TownName, record: Option<Box<Record>> },
    /// The World as a whole: every machine was asked. The directory comes
    /// from the hub while connected, or from available local copies offline.
    World { towns: Vec<TownSummary> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MachineWho {
    Answered {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observation: Option<crate::observation::Observation>,
        reply: WhoReply,
    },
    /// Not connected; `since` is when it was last seen, when known. `last`
    /// is what was afoot on it as it last told, when that is known
    /// (`world-network.md`, 近况).
    Offline {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observation: Option<crate::observation::Observation>,
        machine: MachineName,
        since: Option<Timestamp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last: Option<Box<WhoReply>>,
    },
    /// Connected, or thought to be, but its answer did not come.
    NoAnswer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observation: Option<crate::observation::Observation>,
        machine: MachineName,
        reason: String,
    },
}

/// What a machine knows of one resident or job, for `fastctx w <id>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Looked {
    Resident(Box<ResidentDetail>),
    Job {
        record: Box<JobRecord>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<crate::observation::MachineSource>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResidentDetail {
    pub resident: ResidentNow,
    /// Where its records are, on its machine.
    pub records: String,
    /// Its latest actions, oldest first, and how many it has in all.
    pub actions: Vec<Action>,
    pub actions_total: usize,
    /// For a colleague: what it was asked, and the ends of its latest turns.
    pub asked: Option<String>,
    pub answers: Vec<MessageLine>,
    /// Settled goal reports are separate from actual conversation turns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub goal_reports: Vec<crate::goals::ReportSummary>,
    /// What it started that still runs on its machine.
    pub jobs: Vec<RunningJob>,
}

/// One id looked up across the World: what each machine that has it said,
/// and the machines that could not be asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookedUp {
    pub id: String,
    pub found: Vec<Looked>,
    pub unasked: Vec<String>,
    /// Jobs whose owner could not be looked up, still reported by their execution machine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<RunningJob>,
}

impl WorldWho {
    /// Marks the residents and jobs the machines answered with that work
    /// for none of the reader's accounts, each with whom it works for
    /// (`world-network.md`, 共享机器与借出智能体). `mine` says whether the
    /// reader works for an account; `logged_in` names the accounts logged in
    /// on a machine, for whom a session the user opened there works.
    pub fn mark_others(
        &mut self,
        mine: impl Fn(&crate::access::HubAccount) -> bool,
        logged_in: impl Fn(&MachineName, Option<&crate::observation::MachineSource>) -> Vec<crate::access::HubAccount>,
    ) {
        use crate::access::{Acting, HubAccount};
        let names = |accounts: &[HubAccount]| {
            let mut names: Vec<crate::AccountName> = accounts.iter().map(|account| account.account.clone()).collect();
            names.dedup();
            names
        };
        let mut others = Vec::new();
        for machine in &self.machines {
            let MachineWho::Answered { reply, observation } = machine else { continue };
            let source = observation.as_ref().map(|observation| &observation.source);
            let mut working = std::collections::HashMap::new();
            for resident in &reply.residents {
                let accounts = match &resident.about.acting {
                    Acting::Accounts { accounts } => accounts.clone(),
                    Acting::Here => logged_in(&reply.machine, source),
                };
                if !accounts.is_empty() && !accounts.iter().any(&mine) {
                    let id = resident.about.id.to_string();
                    others.push(WhoFor {
                        source: source.cloned(),
                        machine: reply.machine.clone(),
                        id,
                        accounts: names(&accounts),
                    });
                }
                working.insert(resident.about.id.clone(), accounts);
            }
            for job in &reply.jobs {
                let accounts = match (&job.account, job.owner.resident()) {
                    (Some(account), _) => vec![account.clone()],
                    (None, Some(owner)) if owner.machine == reply.machine => {
                        working.get(&owner.resident).cloned().unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                if !accounts.is_empty() && !accounts.iter().any(&mine) {
                    let id = job.job.to_string();
                    others.push(WhoFor {
                        source: source.cloned(),
                        machine: reply.machine.clone(),
                        id,
                        accounts: names(&accounts),
                    });
                }
            }
        }
        self.others = others;
    }

    /// Merge the history obtained with each machine's current activity.
    pub fn collect_history(&mut self, since: Timestamp, town: Option<TownName>) {
        use crate::since::*;
        let mut history = WorldSince {
            machine: self.machine.clone(),
            since,
            town,
            machines: Vec::new(),
            events: Vec::new(),
            undated: Vec::new(),
            issues: Vec::new(),
            limitations: limitations(),
        };
        for machine in &mut self.machines {
            history.machines.push(match machine {
                MachineWho::Answered { reply, observation } => match reply.history.take() {
                    Some(mut read) => {
                        history.events.append(&mut read.events);
                        history.undated.append(&mut read.undated);
                        MachineSince::Answered { reply: read.coverage, observation: observation.clone() }
                    }
                    None => MachineSince::NoAnswer {
                        machine: reply.machine.clone(),
                        observation: observation.clone(),
                        reason: "answered current activity without the requested history".into(),
                    },
                },
                MachineWho::Offline { machine, since, observation, .. } => {
                    MachineSince::Offline { machine: machine.clone(), since: *since, observation: observation.clone() }
                }
                MachineWho::NoAnswer { machine, reason, observation } => MachineSince::NoAnswer {
                    machine: machine.clone(),
                    reason: reason.clone(),
                    observation: observation.clone(),
                },
            });
        }
        history.events = merge_events(history.events, &mut history.issues);
        let facts = std::mem::take(&mut history.undated);
        crate::observation::union(&mut history.undated, facts);
        self.history = Some(history);
    }
}

impl LookedUp {
    pub fn attach_jobs(&mut self) {
        for found in &mut self.found {
            if let Looked::Resident(detail) = found {
                let owner = crate::Address {
                    resident: detail.resident.about.id.clone(),
                    machine: detail.resident.about.machine.clone(),
                };
                for job in &self.jobs {
                    if job.owner == owner
                        && crate::observation::same_network(job.source.as_ref(), detail.resident.source.as_ref())
                        && !detail.jobs.iter().any(|held| held.machine == job.machine && held.job == job.job)
                    {
                        detail.jobs.push(job.clone());
                    }
                }
                self.jobs.retain(|job| {
                    job.owner != owner
                        || !crate::observation::same_network(job.source.as_ref(), detail.resident.source.as_ref())
                });
                detail.jobs.sort_by_key(|job| (job.machine.clone(), job.started_at));
            }
        }
    }
}

impl CurrentTool {
    pub fn from_item(turn: &str, item: &crate::session::Item) -> Option<Self> {
        use crate::session::{Does, What};
        let What::Tool(tool) = &item.what else { return None };
        let line = |text: &str| {
            let mut lines = text.lines();
            let first = lines.next().unwrap_or_default();
            if lines.next().is_some() { format!("{first} …") } else { first.to_string() }
        };
        let detail = match &tool.does {
            Does::Command { command, .. } => line(command),
            Does::Edit { changes } => changes.iter().map(|change| change.path.as_str()).collect::<Vec<_>>().join(", "),
            Does::Search { query } => line(query),
            Does::Image { path } => path.clone().unwrap_or_default(),
            Does::Colleague { to, task } => task.as_ref().or(to.as_ref()).map(|text| line(text)).unwrap_or_default(),
            Does::Agent { task, kind, agents, .. } => {
                task.as_ref().or(kind.as_ref()).or(agents.first()).map(|text| line(text)).unwrap_or_default()
            }
            Does::Call => ["cmd", "command", "script", "file_path", "path", "query", "url", "description"]
                .into_iter()
                .find_map(|key| {
                    tool.input
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .map(|value| format!("{key}: {}", line(value)))
                })
                .unwrap_or_default(),
        };
        Some(Self {
            turn: turn.to_string(),
            id: item.id.clone(),
            started_at: item.started_at,
            name: tool.name.clone(),
            machine: tool.machine.clone(),
            detail,
        })
    }
}

impl MachineWho {
    pub fn observation(&self) -> Option<&crate::observation::Observation> {
        match self {
            Self::Answered { observation, .. }
            | Self::Offline { observation, .. }
            | Self::NoAnswer { observation, .. } => observation.as_ref(),
        }
    }

    pub fn machine(&self) -> &MachineName {
        match self {
            Self::Answered { reply, .. } => &reply.machine,
            Self::Offline { machine, .. } | Self::NoAnswer { machine, .. } => machine,
        }
    }

    pub fn written(&self) -> crate::access::Written {
        crate::observation::written(self.machine(), self.observation().map(|observation| &observation.source))
    }
}

impl ResidentNow {
    pub fn written(&self) -> crate::access::Written {
        self.source.as_ref().map_or_else(|| (&self.about.machine).into(), |source| source.written.clone())
    }
}

impl RunningJob {
    pub fn written(&self) -> crate::access::Written {
        self.source.as_ref().map_or_else(|| (&self.machine).into(), |source| source.written.clone())
    }
}

impl Looked {
    pub fn id(&self) -> &str {
        match self {
            Self::Resident(detail) => detail.resident.about.id.as_str(),
            Self::Job { record, .. } => record.id.as_str(),
        }
    }

    pub fn written(&self) -> crate::access::Written {
        match self {
            Self::Resident(detail) => detail.resident.written(),
            Self::Job { record, source } => {
                source.as_ref().map_or_else(|| (&record.machine).into(), |source| source.written.clone())
            }
        }
    }
}
