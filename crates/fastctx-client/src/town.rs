//! `fastctx w` as data, for any interface to show (`world-town.md`): who is
//! in a town or in the World, in the order to show them, which machines
//! answered and which could not be asked, and what runs on with nobody here
//! who started it.

use std::collections::BTreeSet;

use fastctx_types::project::{Record, TownSummary};
use fastctx_types::residents::Activity;
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
    present: BTreeSet<Address>,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachineState {
    Answered(MachineName),
    Offline { machine: MachineName, since: Option<Timestamp> },
    NoAnswer { machine: MachineName, reason: String },
}

impl MachineState {
    pub fn machine(&self) -> &MachineName {
        match self {
            MachineState::Answered(machine)
            | MachineState::Offline { machine, .. }
            | MachineState::NoAnswer { machine, .. } => machine,
        }
    }
}

impl TownView {
    /// Shapes the World's answer to `who` for showing, as `you` sees it.
    pub fn new(who: WorldWho, you: Option<&Address>) -> Self {
        let mut machines = Vec::new();
        let mut residents = Vec::new();
        let mut jobs = Vec::new();
        for machine in who.machines {
            match machine {
                MachineWho::Answered { reply } => {
                    machines.push(MachineState::Answered(reply.machine));
                    residents.extend(reply.residents);
                    jobs.extend(reply.jobs);
                }
                MachineWho::Offline { machine, since } => machines.push(MachineState::Offline { machine, since }),
                MachineWho::NoAnswer { machine, reason } => machines.push(MachineState::NoAnswer { machine, reason }),
            }
        }
        jobs.sort_by_key(|job| job.started_at);
        let present = residents.iter().filter(|resident| resident.here).map(address).collect();
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
                            town: name,
                        }
                    })
                    .collect();
                // Across the World, those in a town are counted with it.
                residents.retain(|resident| resident.town.is_none());
                Scope::World { towns: counts }
            }
        };
        residents.sort_by_key(|resident| (Some(&address(resident)) != you, std::cmp::Reverse(latest(resident))));
        Self { machine: who.machine, scope, machines, residents, jobs, present }
    }

    /// How many of those shown are here, and how many busy.
    pub fn counts(&self) -> (usize, usize) {
        let here = self.residents.iter().filter(|resident| resident.here).count();
        let busy = self.residents.iter().filter(|resident| busy(resident)).count();
        (here, busy)
    }

    /// The jobs a resident started that still run.
    pub fn jobs_of(&self, resident: &ResidentNow) -> Vec<&RunningJob> {
        self.jobs_by(&address(resident))
    }

    /// What `owner` started that still runs, on whichever machine.
    pub fn jobs_by(&self, owner: &Address) -> Vec<&RunningJob> {
        self.jobs.iter().filter(|job| job.owner == *owner).collect()
    }

    /// What runs on with nobody here who started it.
    pub fn unattended(&self) -> Vec<&RunningJob> {
        self.jobs.iter().filter(|job| !self.present.contains(&job.owner)).collect()
    }

    /// Whether `machine` answered.
    pub fn answered(&self, machine: &MachineName) -> bool {
        self.machines.iter().any(|state| matches!(state, MachineState::Answered(answered) if answered == machine))
    }
}

/// Where a resident lives.
pub fn address(resident: &ResidentNow) -> Address {
    Address { machine: resident.about.machine.clone(), resident: resident.about.id.clone() }
}

pub fn busy(resident: &ResidentNow) -> bool {
    resident.now.as_ref().is_some_and(|now| now.activity == Activity::Busy)
}

/// When it last did anything the World saw.
pub fn latest(resident: &ResidentNow) -> Timestamp {
    let action = resident.last_action.as_ref().map(|action| action.at);
    let state = resident.now.as_ref().map(|now| now.since);
    action.into_iter().chain(state).max().unwrap_or(resident.about.born_at)
}
