//! Synchronous local write operations shared by tool and executor adapters.
//!
//! Each file is published atomically. Patch sections run in order, as in Codex;
//! an error carries the changes already committed, without rolling them back.

mod atomic;
mod document;
mod edit;
mod patch;
mod storage;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use edit::{edit, write};
pub use patch::apply_patch;

/// Exact replacement in decoded text. Line endings in `old_string` are significant.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EditRequest {
    pub file_path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

/// Create a UTF-8 file, or replace text while retaining the existing encoding and BOM.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WriteRequest {
    pub file_path: String,
    pub content: String,
}

/// The complete Codex patch, including its begin and end markers.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApplyPatchRequest {
    pub input: String,
}

/// Supplies the known source encoding for an absolute file path.
pub type SourceEncoding<'a> = &'a dyn Fn(&Path) -> Option<String>;

/// Caller-owned session and authorization information; never inferred from process cwd.
pub struct WriteContext<'a> {
    /// Execution node label used in the model-facing head note.
    pub node: &'a str,
    /// Lowercase hex SHA-256 of the raw bytes last observed by this session.
    /// Used by `edit` and `write`; patches locate their own current context.
    pub observed_hash: Option<&'a str>,
    /// Optional known source encoding, supplied by a reader or executor for ambiguous files.
    /// The callback receives the absolute requested path, including a move's source path.
    pub source_encoding: Option<SourceEncoding<'a>>,
    /// Return true for directories whose descendants must not be changed.
    /// Checked against both the requested path and resolved symlink targets.
    pub denied_directory: Option<&'a dyn Fn(&Path) -> bool>,
}

impl Default for WriteContext<'_> {
    fn default() -> Self {
        Self {
            node: "local",
            observed_hash: None,
            source_encoding: None,
            denied_directory: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

/// One committed filesystem mutation. Moves report both destination and source.
/// Repeated patch sections retain their individual before/after hashes in order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileChange {
    pub file_path: PathBuf,
    pub kind: ChangeKind,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangedSinceObservation {
    pub file_path: PathBuf,
    pub observed_hash: String,
    pub current_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WriteResult {
    pub text: String,
    pub changes: Vec<FileChange>,
    pub changed_since_observation: Vec<ChangedSinceObservation>,
}

/// Failure plus an exact ledger of earlier successful mutations in this call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WriteError {
    pub message: String,
    pub changes: Vec<FileChange>,
}

impl From<String> for WriteError {
    fn from(message: String) -> Self {
        Self {
            message,
            changes: Vec::new(),
        }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if !self.changes.is_empty() {
            write!(f, "\nChanges already committed:")?;
            for change in &self.changes {
                write!(
                    f,
                    "\n{} {}",
                    change.kind.marker(),
                    change.file_path.display()
                )?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for WriteError {}

impl ChangeKind {
    fn marker(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
        }
    }
}

/// Hash contract shared with readers: SHA-256 over disk bytes, including BOM and line endings.
pub fn content_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn head_note(operation: &str, context: &WriteContext<'_>, changes: usize) -> String {
    format!(
        "=== {operation} on {} ({changes} changes) ===",
        context.node
    )
}
