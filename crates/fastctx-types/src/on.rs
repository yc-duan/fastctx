//! `on`: a part of a script, run on another machine (`world-run.md`,
//! 去别的机器). Everything it sends is collected first; the job there
//! writes its output to that machine's disk, and this side follows it.

use serde::{Deserialize, Serialize};

use crate::run::{EndedHow, ExitStatus};
use crate::{Address, JobId, MachineName, TownName};

/// What `fastctx on` asks of its node: run the package on `machine`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnRequest {
    pub machine: MachineName,
    /// The job on this machine whose script this `on` is part of, when the
    /// World's shell runs it: its answer names where the script went.
    pub job: Option<JobId>,
    pub package: OnPackage,
}

/// The files `fastctx on` collected on this machine, each written whole
/// before the request is made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnPackage {
    /// The script's own variables, functions and changed options, as bash
    /// writes them.
    pub state: String,
    /// The script to run there: the one given on standard input, or the
    /// command line bash quoted from a command's words.
    pub script: String,
    /// The command's standard input, read to its end; `None` for none.
    pub input: Option<String>,
    /// The World's write commands are off in the shell this came from, so
    /// they are off there too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub writes_off: bool,
}

/// How many bytes of each part of a package follow an `On` call on its data
/// connection, in this order, and their SHA-256 together, in hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnSizes {
    pub state: u64,
    pub script: u64,
    pub input: u64,
    pub sha256: String,
}

/// What the machine running an `on` job says on the data connection, after
/// the package, and on a `Follow`'s. Output bytes follow their word.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OnWord {
    /// The complete package could not start a job. Its sender acknowledges
    /// this before the control reply closes the accompanying data stream.
    Refused { failure: crate::ipc::Failure },
    /// The job started, as `job`.
    Started { job: JobId },
    /// `bytes` bytes of standard output follow.
    Out { bytes: u64 },
    /// `bytes` bytes of standard error follow.
    Err { bytes: u64 },
    /// Nothing new for a while: the connection still stands. Said every
    /// `STILL_EVERY` while the job is quiet.
    Still,
    /// Everything it will write has been sent, and this is how it ended.
    /// `lettered`: it had ended while nobody followed, so its end already
    /// went to the resident in a letter, which the sender sets aside.
    Ended {
        end: OnEnd,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        lettered: bool,
    },
}

/// How an `on` job ended, as `on` answers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OnEnd {
    /// The shell exited and its output ended; `remaining` processes it
    /// started still run there, as a background job's do after a script
    /// exits. Older nodes do not say, which reads as none.
    Exited {
        status: ExitStatus,
        #[serde(default, skip_serializing_if = "is_zero")]
        remaining: u32,
    },
    /// It ended otherwise: stopped, interrupted, or never started properly.
    Otherwise { how: EndedHow, status: Option<ExitStatus> },
}

impl OnEnd {
    /// The exit code `on` gives its own caller, as bash reports a
    /// subshell's: a signal as 128 plus its number, and 255 when the World
    /// could not carry the command through (`world-run.md`).
    pub fn code(&self) -> i32 {
        match self {
            OnEnd::Exited { status, .. } | OnEnd::Otherwise { status: Some(status), .. } => code(*status),
            OnEnd::Otherwise { status: None, .. } => 255,
        }
    }
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

fn code(status: ExitStatus) -> i32 {
    match status {
        // A Windows code is 32 bits; bash keeps the low 8 of it.
        ExitStatus::Code(code) => (code & 0xff) as i32,
        ExitStatus::Signal(signal) => 128 + signal,
    }
}

/// What the caller says once it holds the job's end, so the machine that
/// ran it sends no letter about it.
pub const TAKEN: &str = "taken";

/// How often a quiet `on` job says `Still`, and how long its follower waits
/// without hearing anything before it takes the connection for broken: the
/// control line's own heartbeat, ten seconds, and three of them missed
/// (`world-network.md`).
pub const STILL_EVERY: std::time::Duration = std::time::Duration::from_secs(10);
pub const SILENT_FOR: std::time::Duration = std::time::Duration::from_secs(30);

/// Where an `on` was sent from, as the machine running it records it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnOrigin {
    /// The resident that wrote the `on`, at the machine it lives on; the
    /// job is recorded under it however many machines it passed through.
    pub resident: Address,
    pub town: Option<TownName>,
}

/// What the node hands `fastctx on` as the job goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OnPiece {
    Started {
        machine: MachineName,
        job: JobId,
    },
    /// Standard output, base64.
    Out {
        base64: String,
    },
    /// Standard error, base64.
    Err {
        base64: String,
    },
    Ended {
        end: OnEnd,
    },
}
