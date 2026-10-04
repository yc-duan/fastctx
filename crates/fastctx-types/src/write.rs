//! Requests and committed effects of the World's file-writing tools.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EditRequest {
    pub file_path: PathBuf,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WriteRequest {
    pub file_path: PathBuf,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApplyPatchRequest {
    pub input: String,
}

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
    pub path: PathBuf,
    pub kind: ChangeKind,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WriteResult {
    pub text: String,
    pub changes: Vec<FileChange>,
    pub changed_since_seen: Vec<PathBuf>,
}

/// Patch failures keep earlier commits. No rollback can overwrite a later writer.
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
