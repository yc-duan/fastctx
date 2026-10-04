//! Everything a local client can ask the node, and the answers.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::copy::{CopyReport, Place};
use crate::residents::{Delivery, Hints, Host, Landed, Seen, Whereabouts};
use crate::run::{RunReply, RunRequest};
use crate::town::{LookedUp, WorldWho};
use crate::world::NodesReply;
use crate::write::{WriteOp, WriteOutcome};
use crate::{Address, AgentName, JobId, MachineName, ResidentId, TownName};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "call")]
pub enum Call {
    /// A session says who it is; the node finds or makes its resident.
    Arrive { host: Host, hints: Hints },
    /// Run a script for a resident of this machine, on this machine or on
    /// the machine `node` names.
    Run { resident: ResidentId, town: Option<TownName>, request: RunRequest, node: Option<MachineName> },
    /// Writes files for a resident of this machine, on this machine or on
    /// the machine `node` names.
    Write { resident: ResidentId, town: Option<TownName>, op: WriteOp, node: Option<MachineName> },
    /// Copies files or a directory from one machine to another.
    Copy { from: Place, to: Place },
    /// Asks for a colleague for a resident of this machine
    /// (`ask_colleague`): a Codex resident started on this machine or on
    /// the machine `node` names, with `prompt` as its first words. `label`
    /// is the task's few words.
    Ask { resident: ResidentId, town: Option<TownName>, label: String, prompt: String, node: Option<MachineName> },
    /// Words from a resident of this machine to another resident, on any
    /// machine (`message_colleague`); `machine` is where it lives, when the
    /// speaker said. The speaker is owed the end of the turn they go into.
    Message { resident: ResidentId, town: Option<TownName>, to: ResidentId, machine: Option<MachineName>, text: String },
    /// Records what a resident saw of files on this machine through
    /// `fastctx view`; the resident may live on another machine.
    Saw { owner: Address, town: Option<TownName>, seen: Vec<Seen> },
    /// Who is on each machine of the World and what each is doing
    /// (`fastctx w`); with `all`, also the residents who have left.
    Who { all: bool },
    /// One resident or job in detail, by its id, from every machine that
    /// has it (`fastctx w <id>`).
    Look { ids: Vec<String> },
    /// The user's words to a resident on any machine (`fastctx talk`);
    /// `machine` is where it lives, when the user said.
    Talk { to: ResidentId, machine: Option<MachineName>, text: String },
    /// Watches a resident on any machine: the node answers with its history
    /// and then each event as it happens, for as long as the connection
    /// stays open.
    Watch { resident: ResidentId, machine: Option<MachineName> },
    /// Opens a session for the user: a resident of `agent` started on this
    /// machine or on `node`, in `town`, with the user's words as its first.
    Open { agent: AgentName, town: Option<TownName>, node: Option<MachineName>, text: String },
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
    Written { outcome: WriteOutcome },
    Copied(CopyReport),
    Asked { colleague: Address },
    Spoken { to: Address, landed: Landed },
    Opened { resident: Address, landed: Landed },
    Notices { notices: Vec<Delivery> },
    Who(WorldWho),
    Looked { looked: Vec<LookedUp> },
    Joined { world: String, name: MachineName },
    Code { code: String },
    Nodes(NodesReply),
    Done,
}
