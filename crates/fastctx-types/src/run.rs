//! `run`: what a resident asks for, and the record of what it started.
//!
//! A job is one `run` call's script and everything it starts. Its record is
//! a file in the starting resident's directory on the machine where it
//! runs (`world-town.md`, `jobs/`); its supervisor writes it, because the
//! supervisor outlives the node when the node restarts.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Address, JobId, MachineName, ProcessIdentity, TownName};

/// A resident's request to run a bash script.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequest {
    /// The initiating conversation turn, captured before the work starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// The script exactly as the resident wrote it.
    pub script: String,
    /// Where it starts: an absolute path on the machine it runs on. Without
    /// one it starts in that machine's home (`world-run.md`).
    pub cwd: Option<String>,
    /// The session asking has the World's write commands turned off, so
    /// they refuse and point at the host's own (`world-write.md`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub writes_off: bool,
    /// How many seconds the call waits before a job still running goes on
    /// in the background; without it, the World's own wait (`world-run.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<u32>,
}

/// Why a call answered while its job still ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Answered {
    /// The wait was over.
    WaitOver { seconds: u32 },
    /// Words came for the resident after `seconds`, and an answer is the way
    /// they reach it.
    Words { seconds: u32 },
}

/// How a job retains its two output streams. Ordinary `run` uses Merged;
/// a remote subshell uses Separate to preserve stdout as pipeline data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    #[default]
    Merged,
    Separate,
}

/// An intentional stop, attributed by the local node after authenticating
/// its caller. The supervisor authenticates the OS user, not this label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopRequest {
    pub by: String,
    pub reason: String,
}

/// The first stop the supervisor accepted. Present even if stopping later
/// failed; only EndedHow::Stopped establishes successful termination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopRequested {
    pub at: Timestamp,
    pub request: StopRequest,
}

/// What `run` answers: where it ran, how things stood when the wait ended,
/// and a window onto its output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReply {
    /// Actual scratch locations observed while this script ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scratch: Vec<crate::scratch::Location>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Box<crate::run_facts::RunFacts>>,
    /// Completion notices taken by waits of this owner on the executing node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed: Option<Box<Vec<crate::NoticeId>>>,
    pub machine: MachineName,
    pub job: JobId,
    pub state: RunState,
    pub output: OutputWindow,
    /// The machines the script's `on`s went to so far, each with the job it
    /// is there (`world-run.md`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visited: Vec<(MachineName, JobId)>,
    /// Why the call answered, when the job still ran then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<Answered>,
    /// The private places macOS refused Fastctx on that machine, as its node
    /// found once what the command did was refused (`world-gui.md`, 设置).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub privacy: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RunState {
    /// The shell exited and nothing it started runs any more.
    Ended { status: ExitStatus },
    /// The shell exited, but `remaining` processes it started still run;
    /// the job lives on until they end.
    Exited { status: ExitStatus, remaining: u32 },
    /// Still running when the wait ended; it continues in the background.
    Running,
    /// Someone stopped it.
    Stopped,
    /// Its supervisor was gone while it ran (`EndedHow::Interrupted`).
    Interrupted { reason: String },
    /// It could not be started.
    Failed { error: String },
}

/// The part of a job's output shown in one answer. The log holds every
/// byte; the window shows as much as the budget allows, the start and the
/// end when not all of it fits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputWindow {
    /// The file holding the whole output, on the machine it ran on.
    pub log: String,
    /// How many bytes of the log were captured for this window.
    pub bytes: u64,
    /// How many lines the log held then; a last line without a newline
    /// counts.
    pub lines: u64,
    pub pieces: Vec<OutputPiece>,
    /// Shown lines that were not UTF-8 but decoded in the machine's legacy
    /// code page.
    pub legacy: Vec<LegacyLines>,
    /// Shown lines holding bytes that decode in no known encoding; those
    /// bytes appear as `\xNN`.
    pub escaped_lines: u64,
    /// The World's commands in it that failed, each as it was given, in the
    /// order they failed, wherever in the output they did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyLines {
    pub encoding: String,
    pub lines: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OutputPiece {
    /// Text from line `first_line` (counting from 1), as a terminal shows it.
    Text { first_line: u64, text: String },
    /// Characters `from` to `to` (counting from 1) of a line `of`
    /// characters long, when the whole line does not fit.
    PartOfLine { line: u64, from: u64, to: u64, of: u64, text: String },
    /// Lines left out to stay within the budget. A line shown in part on
    /// either side counts as shown.
    Omitted { first_line: u64, last_line: u64 },
    /// An image the output drew with a terminal graphics sequence.
    Image { mime: String, width: u32, height: u32, base64: String },
    /// A graphics sequence that cannot be shown as an image, and why.
    Unreadable { line: u64, reason: String },
}

/// How a program ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitStatus {
    /// It exited with this code. On Windows a code is 32 bits and is kept
    /// unsigned, so a crash reads as `0xC0000005`, not a negative number.
    Code(i64),
    /// A Unix signal ended it.
    Signal(i32),
}

impl std::fmt::Display for ExitStatus {
    /// How a `run` answer says it: `exited 0`, `ended by signal 9`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            // Windows reports crashes as NTSTATUS codes, read in hex.
            ExitStatus::Code(code) if code >= 0x8000_0000 => write!(f, "{EXITED}0x{code:08X}"),
            ExitStatus::Code(code) => write!(f, "{EXITED}{code}"),
            ExitStatus::Signal(signal) => write!(f, "ended by signal {signal}"),
        }
    }
}

const EXITED: &str = "exited ";

/// What the first line of a `run` answer says of how the script stands,
/// read back by whatever shows a session: the code it exited with, or the
/// job it goes on as in the background. The line is
/// `=== run on <machine> (<facts>) ===`, its facts separated by `; `.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunHead {
    pub exit: Option<i64>,
    pub job: Option<String>,
}

impl RunHead {
    pub fn read(line: &str) -> Option<RunHead> {
        let rest = line.strip_prefix("=== run on ")?;
        let (_, facts) = rest.split_once(" (")?;
        let facts = facts.strip_suffix(") ===")?;
        let mut head = RunHead { exit: None, job: None };
        for fact in facts.split("; ") {
            if let Some(code) = fact.strip_prefix(EXITED) {
                head.exit = match code.strip_prefix("0x") {
                    Some(hex) => i64::from_str_radix(hex, 16).ok(),
                    None => code.parse().ok(),
                };
            } else if fact.parse::<JobId>().is_ok() {
                head.job = Some(fact.to_string());
            }
        }
        Some(head)
    }
}

/// The record of one job, kept at `jobs/<id>.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    /// The initiating conversation turn, captured before the work starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub id: JobId,
    pub machine: MachineName,
    /// The resident or open page that started it, including its provenance.
    pub owner: JobOwner,
    /// The resident's town, if any, or the page's project.
    pub town: Option<TownName>,
    pub script: String,
    pub cwd: String,
    /// A complete local input file, or null input when absent. Its producer
    /// must finish and verify it before launch and retain it until job end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    /// The merged log, or stdout alone when `stderr` is present.
    pub output: String,
    /// Separate stderr; absent for ordinary merged `run` jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    pub started_at: Timestamp,
    /// The program put on this job's PATH, reserved before its supervisor
    /// starts. Older records did not identify their program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<JobProgram>,
    /// The supervisor process, once it is running.
    pub supervisor: Option<ProcessIdentity>,
    /// The PID of the shell, once it is running.
    pub pid: Option<u32>,
    /// Set when the shell itself exits; processes it started may still run.
    pub exit: Option<Exited>,
    /// Set after every output writer closed and both streams were drained.
    /// Check the loss fields before treating the logs as complete.
    pub output_ended_at: Option<Timestamp>,
    /// A retained prefix covering every byte queued after the shell exited.
    /// Writers may remain open and append more. Older supervisors only set
    /// `output_ended_at`, which also establishes that the output was drained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_drained: Option<OutputDrained>,
    /// The last observed count after the shell and its output drained; zero
    /// in a successfully finished or stopped record.
    pub remaining: Option<u32>,
    /// Set when the log could not take more output, as when the disk is
    /// full: what failed, and how many bytes the log had kept by then. The
    /// rest of the output was read and dropped, so the job ran on.
    pub output_lost: Option<OutputLost>,
    /// Loss from the separate stderr log, with its own byte count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_lost: Option<OutputLost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopRequested>,
    /// The supervisor's final account. Finished and Stopped establish that
    /// the tree ended; Interrupted and a failure after spawn may not.
    pub end: Option<Ended>,
    /// On Windows the supervisor could not leave the node's job object, so
    /// the job ends when the node ends.
    pub ends_with_node: bool,
    /// The World's write commands refuse in this job (`RunRequest`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub writes_off: bool,
    /// Set for a job an `on` sent here from another machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<OnJob>,
    /// The account a job run here for a resident of another machine works
    /// for, which its end is told as (`world-run.md`, 这里).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<crate::access::HubAccount>,
}

/// Log offsets acknowledged by both output readers after the shell exited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputDrained {
    pub stdout: u64,
    pub stderr: u64,
}

/// What a job an `on` sent here has besides a script (`world-run.md`); its
/// standard input and standard error are the record's `stdin` and `stderr`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnJob {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentinel: Option<crate::on::SentinelProbe>,
    /// The sending script's own state, which the prelude restores first.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<crate::NoticeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follower: Option<MachineName>,
    /// The account the machine that sent it asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<crate::access::HubAccount>,
    /// Its standard output goes straight to the machine of the `on` that
    /// reads it, which takes it from here, and not to its follower.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub piped: bool,
}

/// A resident or an open page that started a job. The resident wire shape
/// remains an address, so previously recorded jobs keep their identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JobOwner {
    Resident(Address),
    Page { page: crate::pages::PageSource },
}

impl From<Address> for JobOwner {
    fn from(address: Address) -> Self {
        Self::Resident(address)
    }
}

impl JobOwner {
    pub fn resident(&self) -> Option<&Address> {
        match self {
            Self::Resident(address) => Some(address),
            Self::Page { .. } => None,
        }
    }

    pub fn tag(&self) -> String {
        match self {
            Self::Resident(address) => address.tag(),
            Self::Page { page } => format!("page:{}:{}:v{}", page.town, page.page, page.version),
        }
    }

    /// The owner as one word that is the same for every spelling of its
    /// machine (`Address::key`), for ids made from it.
    pub fn key(&self) -> String {
        match self {
            Self::Resident(address) => address.key(),
            Self::Page { .. } => self.tag(),
        }
    }
}

impl std::fmt::Display for JobOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resident(address) => write!(f, "{address}"),
            Self::Page { page } => write!(f, "{page}"),
        }
    }
}

impl PartialEq<Address> for JobOwner {
    fn eq(&self, address: &Address) -> bool {
        self.resident() == Some(address)
    }
}

/// The executable a job and its descendants may call for their lifetime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobProgram {
    pub executable: String,
    /// Identifies the boot even if the starter dies before the supervisor
    /// publishes its own identity.
    pub starter: ProcessIdentity,
}

/// A host received an ended job's answer. The output name distinguishes
/// identical short job IDs in different towns without trusting a remote path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobDelivery {
    pub job: JobId,
    pub output: String,
    pub at: Timestamp,
    pub shown_whole: bool,
}

/// An ended job included in an MCP response, awaiting its transport receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedJob {
    pub machine: MachineName,
    pub job: JobId,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputLost {
    pub kept_bytes: u64,
    pub error: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exited {
    pub status: ExitStatus,
    pub at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ended {
    pub at: Timestamp,
    pub how: EndedHow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EndedHow {
    /// Everything it started has exited.
    Finished,
    /// Someone stopped it.
    Stopped,
    /// Its supervisor was gone while it ran, as when the machine restarts;
    /// what happened to its processes after that is not known.
    Interrupted { reason: String },
    /// Starting or supervising failed; processes already started may remain.
    Failed { error: String },
}

impl JobRecord {
    pub fn is_sentinel(&self) -> bool { self.on.as_ref().is_some_and(|on| on.sentinel.is_some()) }
}
