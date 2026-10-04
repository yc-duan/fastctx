//! What `fastctx w` reads: who is here, and what each is doing
//! (`world-town.md`).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::residents::{About, Action};
use crate::{JobId, MachineName, ResidentId, TownName};

/// One machine's answer to `fastctx w`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoReply {
    pub machine: MachineName,
    pub residents: Vec<ResidentNow>,
    /// Jobs still running whose starter has left.
    pub unattended: Vec<RunningJob>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResidentNow {
    pub about: About,
    pub town: Option<TownName>,
    /// Whether a session of the resident is connected now.
    pub here: bool,
    pub last_action: Option<Action>,
    pub running: Vec<RunningJob>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningJob {
    pub job: JobId,
    pub machine: MachineName,
    pub resident: ResidentId,
    pub started_at: Timestamp,
    pub pid: Option<u32>,
    /// The script's first line.
    pub command: String,
    /// The log holding its output, on that machine.
    pub output: String,
}
