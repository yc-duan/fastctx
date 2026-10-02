//! Persistent, caller-independent shell execution. This module has no World or MCP dependency.

mod execution;
mod images;
mod output;
mod platform;
mod png;
mod protocol;
mod record;
#[cfg(test)]
mod tests;
mod window;
#[cfg(windows)]
mod windows_launch;
#[cfg(windows)]
mod windows_record;

pub use execution::{execute, reconcile};
pub use output::{assemble, render};
pub use record::{JobRecord, JobState};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The wait belongs to the caller; it never limits the lifetime of a command or hook.
pub const WAIT: std::time::Duration = std::time::Duration::from_secs(15);
pub const CHARACTER_BUDGET: usize = 150_000;
pub const IMAGE_COST: usize = 5_200;

/// All location and notification policy is supplied by the caller.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    pub command: String,
    pub cwd: PathBuf,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Absolute parent of `out/` and `jobs/`.
    pub directory: PathBuf,
    /// A bash command line; the record's absolute path is appended as one argument.
    pub hook: Option<String>,
    pub machine: String,
}

/// Ordered presentation blocks, deliberately independent of MCP structuredContent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        data: String,
        mime_type: String,
        width: u32,
        height: u32,
    },
}

/// A half-open raw byte range in the log. Offsets do not depend on text encoding.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub job: JobRecord,
    pub record_path: PathBuf,
    /// The immutable snapshot boundary used to assemble this response.
    pub output_bytes: u64,
    pub omitted: Option<ByteRange>,
    pub content: Vec<ContentBlock>,
}

/// Hidden process entry points, separate from the legacy shell job host.
#[doc(hidden)]
pub fn supervise(request: &std::path::Path) -> Result<(), String> {
    execution::supervise(request)
}

#[cfg(unix)]
#[doc(hidden)]
pub fn bootstrap(request: &std::path::Path) -> Result<(), String> {
    platform::bootstrap(request).map_err(|error| error.to_string())
}
