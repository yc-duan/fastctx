//! Everything a local client can ask the node, and the answers.

use serde::{Deserialize, Serialize};

use crate::residents::{Hints, Host, Notice, Whereabouts};
use crate::run::{RunReply, RunRequest};
use crate::{JobId, ResidentId, TownName};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "call")]
pub enum Call {
    /// A session says who it is; the node finds or makes its resident.
    Arrive { host: Host, hints: Hints },
    /// Run a script on this machine for a resident.
    Run { resident: ResidentId, town: Option<TownName>, request: RunRequest },
    /// Takes what waits in a resident's inbox, oldest first.
    Collect { resident: ResidentId, town: Option<TownName> },
    /// A supervisor says its job's record has changed: the shell started
    /// or exited, the output ended, or nothing of the job runs any more.
    JobChanged { job: JobId, resident: ResidentId, town: Option<TownName> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum Reply {
    Arrived(Whereabouts),
    Run(RunReply),
    Notices { notices: Vec<Notice> },
    Done,
}
