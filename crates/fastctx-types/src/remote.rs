//! What one machine asks another to do, and what it sends a resident who
//! lives there (`world-network.md`, 四种话).

use serde::{Deserialize, Serialize};

use crate::copy::{CopyReport, Place};
use crate::ipc::Failure;
use crate::residents::Notice;
use crate::run::{RunReply, RunRequest};
use crate::town::WhoReply;
use crate::world::MachineFacts;
use crate::write::{WriteOp, WriteOutcome};
use crate::{ResidentId, TownName};

/// A call: asked of a machine that is online, answered at once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "call")]
pub enum RemoteCall {
    /// Run a script there for a resident of the calling machine.
    Run { resident: ResidentId, town: Option<TownName>, request: RunRequest },
    /// Write files there for a resident of the calling machine.
    Write { resident: ResidentId, town: Option<TownName>, op: WriteOp },
    /// Who is on that machine and what each is doing, for `fastctx w`.
    Who { all: bool },
    /// What that machine is like, for `fastctx nodes`.
    Facts,
    /// Copy `from`, a path there, to another machine.
    CopySend { from: String, to: Place },
    /// Take a copy from the calling machine, over the call's data
    /// connection: first the manifest, whose size and digest are these, then
    /// the files the receiver asks for. `to` ends in a separator to copy
    /// into that directory.
    CopyReceive { to: String, manifest_bytes: u64, manifest_sha256: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum RemoteReply {
    Run(RunReply),
    Written { outcome: WriteOutcome },
    Who(WhoReply),
    Facts(MachineFacts),
    Copied(CopyReport),
}

/// The answer to a call, as the called machine gave it.
pub type RemoteResult = Result<RemoteReply, Failure>;

/// A letter: something for a resident of another machine, delivered when
/// that machine is reachable, however long that takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "letter")]
pub enum Letter {
    /// Something the World tells the resident `to`.
    Notice { to: ResidentId, notice: Notice },
}
