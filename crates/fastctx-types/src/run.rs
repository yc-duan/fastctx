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
    /// The script exactly as the resident wrote it.
    pub script: String,
    /// Where it starts: an absolute path on the machine it runs on. Without
    /// one it starts in that machine's home (`world-run.md`).
    pub cwd: Option<String>,
}

/// What `run` answers: where it ran, how things stood when the wait ended,
/// and a window onto its output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReply {
    pub machine: MachineName,
    pub job: JobId,
    pub state: RunState,
    pub output: OutputWindow,
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
    /// How many bytes the log held when the window was taken.
    pub bytes: u64,
    /// How many lines the log held then; a last line without a newline
    /// counts.
    pub lines: u64,
    pub pieces: Vec<OutputPiece>,
    /// Shown lines that were not UTF-8 but decoded in the machine's legacy
    /// code page.
    pub legacy: Option<LegacyLines>,
    /// Shown lines holding bytes that decode in no known encoding; those
    /// bytes appear as `\xNN`.
    pub escaped_lines: u64,
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

/// The record of one job, kept at `jobs/<id>.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: JobId,
    pub machine: MachineName,
    /// Who started it, at the machine it lives on, which may be another.
    pub owner: Address,
    /// The town the resident was in, if any.
    pub town: Option<TownName>,
    pub script: String,
    pub cwd: String,
    /// The file holding every byte of its output, on this machine.
    pub output: String,
    pub started_at: Timestamp,
    /// The supervisor process, once it is running.
    pub supervisor: Option<ProcessIdentity>,
    /// The PID of the shell, once it is running.
    pub pid: Option<u32>,
    /// Set when the shell itself exits; processes it started may still run.
    pub exit: Option<Exited>,
    /// Set when no process holds the output open any more: everything the
    /// job will ever print is in the log.
    pub output_ended_at: Option<Timestamp>,
    /// How many processes the shell left running, counted when the shell
    /// had exited and the output had ended.
    pub remaining: Option<u32>,
    /// Set when the log could not take more output, as when the disk is
    /// full: what failed, and how many bytes the log had kept by then. The
    /// rest of the output was read and dropped, so the job ran on.
    pub output_lost: Option<OutputLost>,
    /// Set once nothing of the job runs any more and its output is complete.
    pub end: Option<Ended>,
    /// On Windows the supervisor could not leave the node's job object, so
    /// the job ends when the node ends.
    pub ends_with_node: bool,
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
    /// It could not be started.
    Failed { error: String },
}
