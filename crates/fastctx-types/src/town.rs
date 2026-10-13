//! What `fastctx w` reads: who is here, and what each is doing
//! (`world-town.md`).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::project::{Record, TownSummary};
use crate::residents::{About, Action, MessageLine, Moved, Now};
use crate::run::JobRecord;
use crate::{JobId, MachineName, TownName};

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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResidentNow {
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
        reply: WhoReply,
    },
    /// Not connected; `since` is when it was last seen, when known.
    Offline {
        machine: MachineName,
        since: Option<Timestamp>,
    },
    /// Connected, or thought to be, but its answer did not come.
    NoAnswer {
        machine: MachineName,
        reason: String,
    },
}

/// What a machine knows of one resident or job, for `fastctx w <id>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Looked {
    Resident(Box<ResidentDetail>),
    Job { record: Box<JobRecord> },
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
        logged_in: impl Fn(&MachineName) -> Vec<crate::access::HubAccount>,
    ) {
        use crate::access::{Acting, HubAccount};
        let names = |accounts: &[HubAccount]| {
            let mut names: Vec<crate::AccountName> = accounts.iter().map(|account| account.account.clone()).collect();
            names.dedup();
            names
        };
        let mut others = Vec::new();
        for machine in &self.machines {
            let MachineWho::Answered { reply } = machine else { continue };
            let mut working = std::collections::HashMap::new();
            for resident in &reply.residents {
                let accounts = match &resident.about.acting {
                    Acting::Accounts { accounts } => accounts.clone(),
                    Acting::Here => logged_in(&reply.machine),
                };
                if !accounts.is_empty() && !accounts.iter().any(&mine) {
                    let id = resident.about.id.to_string();
                    others.push(WhoFor { machine: reply.machine.clone(), id, accounts: names(&accounts) });
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
                    others.push(WhoFor { machine: reply.machine.clone(), id, accounts: names(&accounts) });
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
                MachineWho::Answered { reply } => match reply.history.take() {
                    Some(mut read) => {
                        history.events.append(&mut read.events);
                        history.undated.append(&mut read.undated);
                        MachineSince::Answered { reply: read.coverage }
                    }
                    None => MachineSince::NoAnswer {
                        machine: reply.machine.clone(),
                        reason: "answered current activity without the requested history".into(),
                    },
                },
                MachineWho::Offline { machine, since } => {
                    MachineSince::Offline { machine: machine.clone(), since: *since }
                }
                MachineWho::NoAnswer { machine, reason } => {
                    MachineSince::NoAnswer { machine: machine.clone(), reason: reason.clone() }
                }
            });
        }
        history.events = merge_events(history.events, &mut history.issues);
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
                        && !detail.jobs.iter().any(|held| held.machine == job.machine && held.job == job.job)
                    {
                        detail.jobs.push(job.clone());
                    }
                }
                self.jobs.retain(|job| job.owner != owner);
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
