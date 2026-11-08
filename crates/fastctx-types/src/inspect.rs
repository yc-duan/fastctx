//! The read-only looks a machine's node takes, in its own code and never
//! through a program, for the World's own agents: a sentinel's and a goal's
//! judge's only way to see a machine (`world-senses.md`, 哨兵是世界自己的
//! agent). Each is taken by the node of the machine looked at, for an
//! account that may work on it.

use serde::{Deserialize, Serialize};

/// One look at a machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Inspect {
    /// A file: whole, its last `tail` lines, or the lines holding `matching`.
    ReadFile {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matching: Option<String>,
    },
    /// A directory's entries, or those whose names fit `pattern`, where `*`
    /// stands for any run of characters and `?` for one.
    ListDirectory {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
    },
    /// The processes running, or those whose name or command line holds
    /// `matching`.
    Processes {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matching: Option<String>,
    },
    /// The machine's GPUs, memory and disks.
    Resources,
    /// The account's jobs on the machine: those running, and those that
    /// ended within the last day.
    Jobs,
    /// One of the account's jobs' output: its last `tail` lines, or the
    /// lines holding `matching`.
    JobOutput {
        job: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matching: Option<String>,
    },
}

/// What a look saw. `text` is what the machine shows, line by line, and what
/// a reflex's condition is tested against; `note` says what the look did not
/// find or did not show (no such file, no line holding the words, the
/// earlier lines left out), for the model to read and no condition to match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inspected {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The most text one look gives back, in bytes.
pub const MOST_BYTES: usize = 65_536;
/// The most lines a tail or a search gives back.
pub const MOST_LINES: u32 = 2_000;
/// The longest path, words to match and pattern a look takes, in bytes.
const MOST_WORDS: usize = 4_096;

impl Inspect {
    /// Whether it is a look that may be taken, as asked.
    pub fn check(&self) -> Result<(), String> {
        let words = |what: &str, value: &str| {
            if value.trim().is_empty() || value.len() > MOST_WORDS {
                Err(format!("{what} must be given, in at most {MOST_WORDS} bytes"))
            } else {
                Ok(())
            }
        };
        let lines = |tail: Option<u32>, matching: Option<&String>| -> Result<(), String> {
            if tail.is_some_and(|tail| tail == 0 || tail > MOST_LINES) {
                return Err(format!("tail is a count of lines from 1 to {MOST_LINES}"));
            }
            if tail.is_some() && matching.is_some() {
                return Err("give tail or matching, not both".into());
            }
            matching.map_or(Ok(()), |matching| words("matching", matching))
        };
        match self {
            Self::ReadFile { path, tail, matching } => {
                words("path", path)?;
                lines(*tail, matching.as_ref())
            }
            Self::ListDirectory { path, pattern } => {
                words("path", path)?;
                pattern.as_deref().map_or(Ok(()), |pattern| words("pattern", pattern))
            }
            Self::Processes { matching } => matching.as_deref().map_or(Ok(()), |matching| words("matching", matching)),
            Self::Resources | Self::Jobs => Ok(()),
            Self::JobOutput { job, tail, matching } => {
                words("job", job)?;
                lines(*tail, matching.as_ref())
            }
        }
    }
}
