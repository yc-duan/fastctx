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
    /// The hub of `machine`, when the script wrote it as `name.hub`
    /// (`world-network.md`, 机器的名字).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<crate::HubName>,
    /// The client explicitly acknowledges Ended before the node releases
    /// the completion debt. Old clients only consumed the output stream.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub receipt: bool,
    /// The local CLI's OS identity distinguishes its death from an IPC failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<crate::ProcessIdentity>,
    /// The job on this machine whose script this `on` is part of, when the
    /// World's shell runs it: its answer names where the script went.
    pub job: Option<JobId>,
    pub package: OnPackage,
    /// The machine of the `on` the script pipes this one's output into, as
    /// the script wrote it (`world-run.md`, 数据跟着命令走): the output may
    /// go there without passing here, when both are of one hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipe_to: Option<crate::access::Written>,
}

impl OnRequest {
    /// The machine as the script wrote it.
    pub fn written(&self) -> crate::access::Written {
        crate::access::Written { name: self.machine.clone(), hub: self.hub.clone() }
    }
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
    /// The `on` whose output is the command's standard input, when that
    /// output went straight on and stays where it was written until the
    /// machine running this command takes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<OnSource>,
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
    WriteFact {
        fact: Box<crate::run_facts::WriteFact>,
    },
    /// The complete package could not start a job. Its sender acknowledges
    /// this before the control reply closes the accompanying data stream.
    Refused {
        failure: crate::ipc::Failure,
    },
    /// The job started, as `job`.
    Started {
        job: JobId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scratch: Option<String>,
    },
    /// `bytes` bytes of standard output follow.
    Out {
        bytes: u64,
    },
    /// `bytes` bytes of standard error follow.
    Err {
        bytes: u64,
    },
    /// Nothing new for a while: the connection still stands. Said every
    /// `STILL_EVERY` while the job is quiet.
    Still {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scratch: Vec<crate::scratch::Location>,
    },
    /// Everything it will write has been sent, and this is how it ended.
    /// `lettered` is the older protocol's debt-cleared hint. Completion
    /// receipts are now tracked independently of this hint: a lost TAKEN
    /// can produce a later letter even when this field was false.
    Ended {
        end: OnEnd,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scratch: Vec<crate::scratch::Location>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        lettered: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        consumed: Vec<crate::NoticeId>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        through_exit: bool,
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

/// The CLI's downstream reader deliberately stopped consuming output.
pub const STOP: &str = "stop";

/// How often a quiet `on` job says `Still`, and how long its follower waits
/// without hearing anything before it takes the connection for broken: the
/// control line's own heartbeat, ten seconds, and three of them missed
/// (`world-network.md`).
pub const STILL_EVERY: std::time::Duration = std::time::Duration::from_secs(10);
pub const SILENT_FOR: std::time::Duration = std::time::Duration::from_secs(30);

/// Where an `on` was sent from, as the machine running it records it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnOrigin {
    /// The initiating conversation turn, captured before the work starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    /// Internal observations retain their identity through execution and recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentinel: Option<SentinelProbe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// The resident that wrote the `on`, at the machine it lives on; the
    /// job is recorded under it however many machines it passed through.
    pub resident: Address,
    pub town: Option<TownName>,
    /// One dispatch, known before Started. An intentional stop can name it
    /// even when the job id did not get back to the sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<crate::NoticeId>,
    /// The machine whose CLI follows this hop, which may differ from the resident's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follower: Option<MachineName>,
    /// The account the hop runs for, as the machine it runs on was called
    /// for: set there, from the call, never taken from the sender's word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<crate::access::HubAccount>,
}

/// Where the output of an `on` that went straight on is kept: the job on
/// that machine, and the dispatch it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnSource {
    pub machine: MachineName,
    pub job: JobId,
    pub request: crate::NoticeId,
}

/// What an `on` whose output went straight on writes on its standard
/// output, as the one line there, for the `on` that reads it: this, then
/// the source as JSON.
const PIPE_MARK: &str = "\u{1b}fastctx on output ";

/// The most a line of `PIPE_MARK` takes: the longest names and ids fit.
pub const PIPE_LINE_MAX: u64 = 512;

impl OnSource {
    pub fn line(&self) -> String {
        format!("{PIPE_MARK}{}\n", serde_json::to_string(self).expect("sources serialize"))
    }

    /// The source `input` names, when it is nothing but such a line.
    pub fn read(input: &[u8]) -> Option<Self> {
        let json = std::str::from_utf8(input).ok()?.strip_prefix(PIPE_MARK)?.strip_suffix('\n')?;
        if json.contains('\n') {
            return None;
        }
        serde_json::from_str(json).ok()
    }
}

/// What the machine an `on`'s output went straight on from says as another
/// machine takes it: the bytes as they are written, `Still` while there are
/// none, and at the end how many there were and their SHA-256, in hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PipeWord {
    Out { bytes: u64 },
    Still,
    End { bytes: u64, sha256: String },
}

/// What the node hands `fastctx on` as the job goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OnPiece {
    Started {
        machine: MachineName,
        job: JobId,
    },
    /// The job's standard output goes to the `on` reading it, from where
    /// it is kept: this is all of it that comes here.
    Piped {
        source: OnSource,
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

/// An internal observation, distinct from the owner's ordinary shell work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentinelProbe {
    pub id: crate::SentinelId,
    pub generation: u64,
}
