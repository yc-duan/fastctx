//! Requests and committed effects of the World's file-writing tools.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EditRequest {
    pub file_path: PathBuf,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

/// Ordered SEARCH/REPLACE blocks, committed together to one file.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EditBlocksRequest {
    pub file_path: PathBuf,
    pub input: String,
    #[serde(default)]
    pub replace_all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WriteRequest {
    pub file_path: PathBuf,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ApplyPatchRequest {
    pub input: String,
}

/// A write as a resident asks for it, through whichever of the tools its
/// host has (`world-write.md`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "tool")]
pub enum WriteOp {
    Edit(EditRequest),
    EditBlocks(EditBlocksRequest),
    Write(WriteRequest),
    ApplyPatch(ApplyPatchRequest),
}

impl WriteOp {
    /// The tool's name, as the resident's actions record it.
    pub fn tool(&self) -> &'static str {
        match self {
            WriteOp::Edit(_) | WriteOp::EditBlocks(_) => "edit",
            WriteOp::Write(_) => "write",
            WriteOp::ApplyPatch(_) => "apply_patch",
        }
    }
}

/// How a write came out: what it changed, or why it stopped and what it had
/// changed by then.
pub type WriteOutcome = Result<WriteResult, WriteError>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

impl ChangeKind {
    pub fn marker(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
        }
    }
}

/// One committed path mutation. A move records destination publication and source
/// deletion separately; repeated patch sections retain their execution order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<Box<crate::run_facts::WriteFact>>,
    pub path: PathBuf,
    pub kind: ChangeKind,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    /// The encoding actually used for the published bytes, absent for deletions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    /// The explicit selection actually used for this revision, not an inferred label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_selection: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WriteResult {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub note_prefix: usize,
    pub text: String,
    pub changes: Vec<FileChange>,
    pub changed_since_seen: Vec<PathBuf>,
}

/// Patch failures keep earlier commits. No rollback can overwrite a later writer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WriteError {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub note_prefix: usize,
    pub message: String,
    pub changes: Vec<FileChange>,
}

impl From<String> for WriteError {
    fn from(message: String) -> Self {
        Self { message, changes: Vec::new(), note_prefix: 0 }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)?;
        if !self.changes.is_empty() {
            f.write_str("\nChanges already committed:")?;
            for change in &self.changes {
                write!(f, "\n{} {}", change.kind.marker(), change.path.display())?;
            }
        }
        Ok(())
    }
}
impl std::error::Error for WriteError {}

fn is_zero(value: &usize) -> bool {
    *value == 0
}
