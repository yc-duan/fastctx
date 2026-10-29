//! What a run did through the World's own commands, step by step
//! (`world-gui.md`, 对话): a command sent to another machine, files copied
//! or looked at, jobs waited for or stopped, a page published, a project
//! entered, the plan written, a goal marked, a sentinel set. Each is told
//! from inside the job whose script gave it, by the node that relays the
//! `on` or by the command itself, so a conversation draws what was done
//! rather than reading the words its model was given.

use crate::run::{EndedHow, ExitStatus, JobRecord};
use crate::{JobId, MachineName, TownName};
use serde::{Deserialize, Serialize};

/// The longest a step's text keeps of a command, a tail or a problem.
pub const STEP_TEXT: usize = 4096;
/// The most entries a step keeps of a list.
pub const STEP_ENTRIES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStep {
    /// Unique among the run's steps; a later telling of the same id
    /// replaces an earlier, as an `on` that began and then ended.
    pub id: String,
    /// The job whose script gave the command: the run's own, or that of an
    /// `on` it sent to another machine.
    pub job: JobId,
    /// The machine that job runs on.
    pub machine: MachineName,
    pub began: jiff::Timestamp,
    /// Unset while it goes on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<jiff::Timestamp>,
    #[serde(flatten)]
    pub did: StepDid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StepDid {
    /// A command sent to `to` with `on`, run there as the job `sent`.
    On {
        to: MachineName,
        /// The command line as it was given, or the script it carried.
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sent: Option<JobId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end: Option<StepEnd>,
        /// The machine of the `on` its output is piped into.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pipes_to: Option<MachineName>,
        /// The job whose output came straight in as its input, machine to
        /// machine.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fed_by: Option<JobId>,
        /// Its last lines as they came back, at most twenty.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail: Option<String>,
    },
    Copy {
        from: Vec<StepPlace>,
        to: StepPlace,
        files: u64,
        bytes: u64,
        /// Files there already with the same contents.
        unchanged: u64,
        /// What was not copied, and why, as the copy said it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        problems: Vec<String>,
    },
    /// Files looked at with `fastctx view` on the step's machine; the
    /// pictures it gave, in the order of the images in the call's output.
    View {
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pictures: Vec<StepPicture>,
    },
    /// Jobs waited for with `fastctx wait`, as each ended.
    Wait {
        jobs: Vec<WaitedJob>,
    },
    /// A job, a colleague's turn or a sentinel stopped with `fastctx stop`.
    Stop {
        /// What was named: a job, a colleague or a sentinel.
        what: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<MachineName>,
        how: StopHow,
    },
    Page {
        name: String,
        town: TownName,
        title: String,
        version: u64,
    },
    Project {
        town: TownName,
    },
    /// The conversation's plan written whole.
    Plan {
        version: u64,
    },
    Goal {
        mark: GoalMark,
    },
    Sense {
        sentinel: String,
        condition: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepPlace {
    pub machine: MachineName,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepPicture {
    pub path: String,
    /// The page of a document drawn as a picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub mime: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitedJob {
    pub job: JobId,
    pub machine: MachineName,
    /// When it ended.
    pub at: jiff::Timestamp,
    pub end: StepEnd,
    /// Its last lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
    /// The script it ran, its beginning where it is long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// How what a step started or waited for ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StepEnd {
    /// It exited with `code`, a signal as 128 and its number.
    Exited {
        code: i64,
    },
    Stopped,
    /// It did not run through: its machine went away, or its supervisor.
    Lost {
        why: String,
    },
    /// It never started.
    NotRun {
        why: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopHow {
    Stopped,
    /// Someone had stopped it already, or there was nothing to stop.
    AlreadyStopped,
    HadEnded,
    NotCleanly,
    StillRunning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalMark {
    Complete,
    Blocked,
    Pause,
}

impl StepEnd {
    /// How a job's record says it ended; None while it runs.
    pub fn of_record(record: &JobRecord) -> Option<Self> {
        let end = record.end.as_ref()?;
        Some(Self::of(&end.how, record.exit.map(|exit| exit.status)))
    }

    pub fn of(how: &EndedHow, status: Option<ExitStatus>) -> Self {
        match how {
            EndedHow::Finished => Self::Exited { code: status.map_or(0, code) },
            EndedHow::Stopped => Self::Stopped,
            EndedHow::Interrupted { reason } => Self::Lost { why: clipped(reason) },
            EndedHow::Failed { error } => Self::Lost { why: clipped(error) },
        }
    }
}

impl StopHow {
    /// How a stop left the job its record describes.
    pub fn of_record(record: &JobRecord) -> Self {
        match record.end.as_ref().map(|end| &end.how) {
            Some(EndedHow::Stopped) if record.stop.is_some() => Self::Stopped,
            Some(EndedHow::Stopped) => Self::AlreadyStopped,
            Some(EndedHow::Finished | EndedHow::Interrupted { .. }) => Self::HadEnded,
            Some(EndedHow::Failed { .. }) => Self::NotCleanly,
            None => Self::StillRunning,
        }
    }
}

/// The exit code bash gives for a status: a signal as 128 and its number.
fn code(status: ExitStatus) -> i64 {
    match status {
        ExitStatus::Code(code) => code,
        ExitStatus::Signal(signal) => 128 + i64::from(signal),
    }
}

/// `text` kept to `STEP_TEXT` bytes, cut at a character.
pub fn clipped(text: &str) -> String {
    if text.len() <= STEP_TEXT {
        return text.to_string();
    }
    let mut end = STEP_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// The last `lines` lines of `text`, kept to `STEP_TEXT` bytes from its end.
pub fn tail_of(text: &str, lines: usize) -> String {
    let trimmed = text.trim_end_matches(['\n', '\r']);
    let start = trimmed.rmatch_indices('\n').nth(lines.saturating_sub(1)).map_or(0, |(at, _)| at + 1);
    let mut tail = &trimmed[start..];
    if tail.len() > STEP_TEXT {
        let mut from = tail.len() - STEP_TEXT;
        while !tail.is_char_boundary(from) {
            from += 1;
        }
        tail = &tail[from..];
    }
    tail.to_string()
}

impl RunStep {
    /// Whether the step keeps within what a run's facts carry of one.
    pub fn bounded(&self) -> bool {
        let short = |text: &str| text.len() <= STEP_TEXT;
        let few = |count: usize| count <= STEP_ENTRIES;
        !self.id.is_empty()
            && self.id.len() <= 64
            && self.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && match &self.did {
                StepDid::On { command, tail, .. } => short(command) && tail.as_deref().is_none_or(short),
                StepDid::Copy { from, to, problems, .. } => {
                    few(from.len())
                        && from.iter().all(|place| short(&place.path))
                        && short(&to.path)
                        && few(problems.len())
                        && problems.iter().all(|problem| short(problem))
                }
                StepDid::View { paths, pictures } => {
                    few(paths.len())
                        && paths.iter().all(|path| short(path))
                        && few(pictures.len())
                        && pictures.iter().all(|picture| short(&picture.path) && short(&picture.mime))
                }
                StepDid::Wait { jobs } => {
                    few(jobs.len())
                        && jobs.iter().all(|job| {
                            job.tail.as_deref().is_none_or(short) && job.command.as_deref().is_none_or(short)
                        })
                }
                StepDid::Stop { what, .. } => short(what),
                StepDid::Page { name, title, .. } => short(name) && short(title),
                StepDid::Project { .. } | StepDid::Plan { .. } | StepDid::Goal { .. } => true,
                StepDid::Sense { sentinel, condition } => short(sentinel) && short(condition),
            }
    }

    /// Whether `self`, told of the same step, says more than `other`: what
    /// ended says more than what goes on.
    pub fn supersedes(&self, other: &Self) -> bool {
        self.ended.is_some() || other.ended.is_none()
    }
}

/// `incoming` merged into `steps` by id, an ended step kept over the same
/// step going on; new steps after the known ones.
pub fn merge_steps(steps: &mut Vec<RunStep>, incoming: &[RunStep]) {
    for step in incoming {
        match steps.iter_mut().find(|known| known.id == step.id) {
            Some(known) => {
                if step.supersedes(known) {
                    *known = step.clone();
                }
            }
            None => steps.push(step.clone()),
        }
    }
}
