//! What `fastctx w` reads: who is here, and what each is doing
//! (`world-town.md`).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::residents::{About, Action};
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
    /// Whether a session of the resident is connected now.
    pub here: bool,
    pub last_action: Option<Action>,
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

/// `fastctx w` across the World: each machine's answer, or why there is none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldWho {
    /// The machine that asked.
    pub machine: MachineName,
    pub machines: Vec<MachineWho>,
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
