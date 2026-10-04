//! Everything a local client can ask the node, and the answers.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::residents::{Hints, Host, Notice, Seen, Whereabouts};
use crate::run::{RunReply, RunRequest};
use crate::town::WorldWho;
use crate::world::NodesReply;
use crate::{Address, JobId, MachineName, ResidentId, TownName};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "call")]
pub enum Call {
    /// A session says who it is; the node finds or makes its resident.
    Arrive { host: Host, hints: Hints },
    /// Run a script for a resident of this machine, on this machine or on
    /// the machine `node` names.
    Run { resident: ResidentId, town: Option<TownName>, request: RunRequest, node: Option<MachineName> },
    /// Records what a resident saw of files on this machine through
    /// `fastctx view`; the resident may live on another machine.
    Saw { owner: Address, town: Option<TownName>, seen: Vec<Seen> },
    /// Who is on each machine of the World and what each is doing
    /// (`fastctx w`); with `all`, also the residents who have left.
    Who { all: bool },
    /// Takes what waits in a resident's inbox, oldest first.
    Collect { resident: ResidentId, town: Option<TownName> },
    /// A supervisor says its job's record has changed: the shell started
    /// or exited, the output ended, or nothing of the job runs any more.
    JobChanged { job: JobId, owner: Address, town: Option<TownName> },
    /// Joins this machine to a World with a code, under `name` or the name
    /// it has.
    Join { code: String, name: Option<MachineName> },
    /// Makes an invitation for one more machine.
    Invite { name: Option<MachineName>, expires: Timestamp },
    /// Revokes a member's key, so no machine trusts it any more.
    Revoke { name: MachineName, reason: String },
    /// This machine leaves its World and becomes a World of one again.
    Leave,
    /// This machine's link and every machine of the World (`fastctx nodes`).
    Nodes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum Reply {
    Arrived(Whereabouts),
    Run(RunReply),
    Notices { notices: Vec<Notice> },
    Who(WorldWho),
    Joined { world: String, name: MachineName },
    Code { code: String },
    Nodes(NodesReply),
    Done,
}
