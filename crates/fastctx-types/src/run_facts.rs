//! Small, durable facts of a World tool call; large artifacts stay where they were made.

use crate::run::RunState;
use crate::write::ChangeKind;
use crate::{JobId, MachineName};
use serde::{Deserialize, Serialize};

mod steps;
pub use steps::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFacts {
    /// The executor's complete job record, including the original script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// The initiating conversation turn, captured before the work starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    pub machine: MachineName,
    pub state: RunState,
    pub job: Option<JobId>,
    pub output: Option<OutputRef>,
    pub visited: Vec<(MachineName, JobId)>,
    pub writes: Vec<WriteFact>,
    /// The same selected output as the model's, with its World notes identified.
    pub spans: Vec<OutputSpan>,
    /// All exact matches in the retained capture, including the model's omitted middle.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<OutputNoteMatch>,
    /// What it did through the World's own commands, in the order told.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<RunStep>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<crate::session::Fingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    pub machine: MachineName,
    pub path: String,
    pub bytes: u64,
    pub lines: u64,
    pub cut: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFact {
    /// The initiating conversation turn, captured before the work starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    pub id: String,
    pub machine: MachineName,
    pub path: String,
    pub kind: ChangeKind,
    /// Null when bytes do not have a supported, unambiguous line decoding.
    pub added: Option<u32>,
    pub removed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    pub diff: Option<DiffRef>,
    /// The exact UTF-8 bytes of the command's stdout note, including its ending.
    /// A failed writer emits to stderr and therefore has no stdout note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<WriteNote>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WriteNote {
    Text {
        command: NoteCommand,
        text: String,
    },
    /// Another mutation of the same operation holds the exact bytes once.
    Same {
        fact: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<crate::session::Fingerprint>,
    pub machine: MachineName,
    /// UTF-8 JSON lines, one DiffLine per displayed row.
    pub path: String,
    /// How many bytes those lines take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// The rows themselves, for a change whose rows take at most
    /// `INLINE_DIFF` bytes: kept with the conversation, as what was said
    /// is, they are shown with no request (`world-gui.md`, 对话).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<DiffLine>>,
}

/// The most bytes of rows a change keeps with the conversation; about
/// ninety-nine changes in a hundred take fewer.
pub const INLINE_DIFF: u64 = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub text: String,
    /// Exact decoded line ending: empty for an unterminated final line.
    pub ending: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    Hunk,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSpan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<OutputNote>,
    pub piece: DisplayPiece,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DisplayPiece {
    Text {
        first_line: u64,
        text: String,
    },
    PartOfLine {
        line: u64,
        /// One-based character coordinates, excluding the line ending. A retained
        /// newline with no retained text has an empty range: from == to + 1.
        from: u64,
        to: u64,
        of: u64,
        text: String,
    },
    Omitted {
        first_line: u64,
        last_line: u64,
    },
    /// Index among the image blocks of the host's original tool result, without another copy.
    Image {
        image: u32,
        mime: String,
        width: u32,
        height: u32,
    },
    Unreadable {
        line: u64,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputNote {
    pub command: NoteCommand,
    /// Identifies one writer operation; patch facts share its prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputNoteMatch {
    pub note: OutputNote,
    /// Zero-based raw byte offsets, end exclusive; not decoded viewer coordinates.
    pub from: u64,
    pub to: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteCommand {
    Write,
    Edit,
    ApplyPatch,
    View,
    Nodes,
    Accounts,
    Who,
    Wait,
    Stop,
    Copy,
    Project,
    Journal,
    Page,
    On,
    Error,
}

/// An executor's incremental facts; original model output is never replaced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<jiff::Timestamp>,
    /// The initiating conversation turn, captured before the work starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    pub call_id: String,
    pub primary: bool,
    pub output: Option<OutputRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<OutputNoteMatch>>,
    pub visited: Vec<(MachineName, JobId)>,
    pub machine: MachineName,
    pub job: JobId,
    pub sequence: u64,
    pub state: Option<RunState>,
    pub writes: Vec<WriteFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<RunStep>,
}
