//! What `fastctx w` reads: who is here, and what each is doing
//! (`world-town.md`).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::project::{Record, TownSummary};
use crate::residents::{About, Action, MessageLine, Moved, Now};
use crate::run::JobRecord;
use crate::{Address, JobId, MachineName, TownName};

/// One machine's answer to `fastctx w`: the residents that live on it, and
/// every job still running on it, whoever started it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoReply {
    pub machine: MachineName,
    pub residents: Vec<ResidentNow>,
    pub jobs: Vec<RunningJob>,
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
    /// It lived in `town` and moved away: what it did there stays there.
    #[serde(default)]
    pub moved: Option<Moved>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningJob {
    pub job: JobId,
    /// Where it runs.
    pub machine: MachineName,
    pub owner: Address,
    pub started_at: Timestamp,
    pub pid: Option<u32>,
    /// The script's first line.
    pub command: String,
    /// The log holding its output, on that machine.
    pub output: String,
}

/// `fastctx w` in a town or across the World: each machine's answer, or why
/// there is none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldWho {
    /// The machine that asked.
    pub machine: MachineName,
    pub scope: WhoScope,
    pub machines: Vec<MachineWho>,
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
}
