//! `fastctx w` as data, for any interface to show (`world-town.md`): who is
//! in a town or in the World, in the order to show them, which machines
//! answered and which could not be asked, and what runs on with nobody here
//! who started it.

use std::collections::{BTreeMap, BTreeSet};

use fastctx_types::access::{HubId, Written};
use fastctx_types::observation::MachineSource;
use fastctx_types::project::{Record, TownSummary};
use fastctx_types::residents::{Activity, Origin};
use fastctx_types::town::{MachineWho, ResidentNow, RunningJob, WhoScope, WorldWho};
use fastctx_types::{Address, MachineName, TownName};
use jiff::Timestamp;

/// One look at a town, or at the World.
#[derive(Clone, Debug)]
pub struct TownView {
    /// The machine that asked.
    pub machine: MachineName,
    pub scope: Scope,
    /// Each machine asked, and how it answered.
    pub machines: Vec<MachineState>,
    /// The residents shown: in a town, everyone there; across the World,
    /// those in no town. The caller first, then whoever acted last.
    pub residents: Vec<ResidentNow>,
    /// Every job still running on the machines that answered.
    pub jobs: Vec<RunningJob>,
    /// Everyone here on the machines that answered, shown or not.
    present: BTreeSet<(Option<HubId>, Address)>,
    known: BTreeMap<(Option<HubId>, Address), ResidentNow>,
    /// Account answers missing from otherwise successful machine replies.
    pub unanswered: Vec<String>,
    /// Whom the residents and jobs that work for other accounts than the
    /// asker's work for, by machine and id.
    others: BTreeMap<(Option<HubId>, MachineName, String), Vec<fastctx_types::AccountName>>,
    /// The machines the asking conversation involves.
    pub involved: BTreeSet<MachineName>,
}

#[derive(Clone, Debug)]
pub enum Scope {
    Town {
        town: TownName,
        record: Option<Box<Record>>,
    },
    /// Across the World: each town with how many are here and busy in it.
    World {
        towns: Vec<TownCount>,
    },
}

#[derive(Clone, Debug)]
pub struct TownCount {
    pub town: TownName,
    /// From this machine's copy of its ledger; `None` when it keeps none.
    pub summary: Option<TownSummary>,
    pub here: usize,
    pub busy: usize,
    pub running: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachineState {
    Answered(Written),
    Offline { machine: Written, since: Option<Timestamp> },
    NoAnswer { machine: Written, reason: String },
}

impl MachineState {
    pub fn machine(&self) -> &MachineName {
        match self {
            MachineState::Answered(machine)
            | MachineState::Offline { machine, .. }
            | MachineState::NoAnswer { machine, .. } => &machine.name,
        }
    }
}

impl TownView {
    /// Shapes the World's answer to `who` for showing, as `you` sees it.
    pub fn new(who: WorldWho, you: Option<&Address>) -> Self {
        let mut machines = Vec::new();
        let mut residents = Vec::new();
        let mut jobs = Vec::new();
        let others = who
            .others
            .into_iter()
            .map(|other| ((hub(other.source.as_ref()), other.machine, other.id), other.accounts))
            .collect();
        let mut unanswered = Vec::new();
        let involved = who.involved.into_iter().collect();
        for machine in who.machines {
            let written = machine.written();
            // A machine offline or silent is said so once; an account that
            // did not answer on one that answered is said apart.
            if matches!(machine, MachineWho::Answered { .. })
                && let Some(observation) = machine.observation()
            {
                unanswered.extend(observation.unanswered.iter().map(|missing| format!("{written} ({missing})")));
            }
            match machine {
                MachineWho::Answered { reply, .. } => {
                    machines.push(MachineState::Answered(written));
                    residents.extend(reply.residents);
                    jobs.extend(reply.jobs);
                }
                MachineWho::Offline { since, .. } => machines.push(MachineState::Offline { machine: written, since }),
                MachineWho::NoAnswer { reason, .. } => {
                    machines.push(MachineState::NoAnswer { machine: written, reason })
                }
            }
        }
        jobs.sort_by_key(|job| (job.machine.clone(), job.started_at));
        let known = residents.iter().map(|r| ((hub(r.source.as_ref()), address(r)), r.clone())).collect();
        let present = residents
            .iter()
            .filter(|resident| resident.here)
            .map(|resident| (hub(resident.source.as_ref()), address(resident)))
            .collect();
        let scope = match who.scope {
            WhoScope::Town { town, record } => Scope::Town { town, record },
            WhoScope::World { towns } => {
                let mut names: BTreeSet<TownName> = towns.iter().map(|town| town.town.clone()).collect();
                names.extend(residents.iter().filter_map(|resident: &ResidentNow| resident.town.clone()));
                let counts = names
                    .into_iter()
                    .map(|name| {
                        let living: Vec<&ResidentNow> =
                            residents.iter().filter(|resident| resident.town.as_ref() == Some(&name)).collect();
                        TownCount {
                            summary: towns.iter().find(|town| town.town == name).cloned(),
                            here: living.iter().filter(|resident| resident.here).count(),
                            busy: living.iter().filter(|resident| busy(resident)).count(),
                            running: jobs.iter().filter(|job| job.town.as_ref() == Some(&name)).count()
                                + living.iter().filter(|r| r.now.as_ref().is_some_and(|now| now.desk)).count(),
                            town: name,
                        }
                    })
                    .collect();
                // Across the World, those in a town are counted with it.
                residents.retain(|resident| resident.town.is_none());
                jobs.retain(|job| job.town.is_none());
                Scope::World { towns: counts }
            }
        };
        residents.sort_by_key(|resident| (Some(&address(resident)) != you, std::cmp::Reverse(latest(resident))));
        Self { machine: who.machine, scope, machines, residents, jobs, present, known, others, unanswered, involved }
    }

    /// Whom the resident or job `id` on `machine` works for, when that is
    /// other accounts than the asker's: `bob`, or `bob, carol`.
    pub fn works_for(&self, machine: &MachineName, id: &str) -> Option<String> {
        let mut matches = self
            .others
            .iter()
            .filter(|((_, name, known), _)| name == machine && known == id)
            .map(|(_, accounts)| accounts);
        let accounts = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(accounts.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))
    }

    pub fn works_for_in(&self, machine: &MachineName, id: &str, source: Option<&MachineSource>) -> Option<String> {
        let accounts = self.others.get(&(hub(source), machine.clone(), id.to_string()))?;
        Some(accounts.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))
    }

    pub fn known(&self, address: &Address) -> Option<&ResidentNow> {
        let mut matches = self.known.iter().filter(|((_, known), _)| known == address).map(|(_, resident)| resident);
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    /// How many of those shown are here, and how many busy.
    pub fn counts(&self) -> (usize, usize) {
        let here = self.residents.iter().filter(|resident| resident.here).count();
        let busy = self.residents.iter().filter(|resident| busy(resident)).count();
        (here, busy)
    }

    /// The jobs a resident started that still run.
    pub fn jobs_of(&self, resident: &ResidentNow) -> Vec<&RunningJob> {
        self.jobs
            .iter()
            .filter(|job| job.owner == address(resident) && hub(job.source.as_ref()) == hub(resident.source.as_ref()))
            .collect()
    }

    /// What `owner` started that still runs, on whichever machine.
    pub fn jobs_by(&self, owner: &Address) -> Vec<&RunningJob> {
        self.jobs.iter().filter(|job| job.owner == *owner).collect()
    }

    /// The colleagues `asker` asked for, among those shown.
    pub fn colleagues_of(&self, asker: &Address) -> Vec<&ResidentNow> {
        self.residents
            .iter()
            .filter(
                |resident| matches!(&resident.about.origin, Origin::Colleague { asked_by, .. } if asked_by == asker),
            )
            .collect()
    }

    /// What runs on with nobody here who started it.
    pub fn unattended(&self) -> Vec<&RunningJob> {
        self.jobs
            .iter()
            .filter(|job| {
                job.owner
                    .resident()
                    .is_none_or(|owner| !self.present.contains(&(hub(job.source.as_ref()), owner.clone())))
            })
            .collect()
    }

    /// Whether `machine` answered.
    /// Whether `machine` is one of those this view was made of: one the
    /// caller may use, answered or not.
    pub fn shows(&self, machine: &MachineName) -> bool {
        self.machines.iter().any(|state| state.machine() == machine)
    }

    pub fn answered(&self, machine: &MachineName) -> bool {
        self.machines.iter().any(|state| matches!(state, MachineState::Answered(answered) if &answered.name == machine))
    }
}

/// Where a resident lives.
pub fn address(resident: &ResidentNow) -> Address {
    Address { machine: resident.about.machine.clone(), resident: resident.about.id.clone() }
}

pub fn busy(resident: &ResidentNow) -> bool {
    resident.here
        && resident.moved.is_none()
        && resident.removed.is_none()
        && resident.now.as_ref().is_some_and(|now| now.activity == Activity::Busy)
}

/// When it last did anything the World saw.
pub fn latest(resident: &ResidentNow) -> Timestamp {
    let action = resident.last_action.as_ref().map(|action| action.at);
    let state = resident.now.as_ref().map(|now| now.last_tool_at.map_or(now.since, |tool| tool.max(now.since)));
    action
        .into_iter()
        .chain(state)
        .chain(resident.tools.iter().filter_map(|tool| tool.started_at))
        .max()
        .unwrap_or(resident.about.born_at)
}

fn hub(source: Option<&MachineSource>) -> Option<HubId> {
    source.map(|source| source.hub.clone())
}
