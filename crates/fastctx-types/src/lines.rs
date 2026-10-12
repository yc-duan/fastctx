//! Logical lines for the user's long-output views. Offsets are UTF-16 units.

use crate::{JobId, MachineName};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Decoding {
    File {
        encoding: Option<String>,
    },
    Output {
        job: JobId,
    },
    /// A history's immutable artifact; reject a changed file before serving it.
    Immutable {
        content: crate::session::Fingerprint,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotation: Option<String>,
        decoding: Box<Decoding>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extent {
    pub length: u64,
    pub complete: bool,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u64,
    /// Omitted selects the start going forward and the end going backward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    pub line: u64,
    pub offset: u64,
    pub length: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Forward,
    Backward,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Request {
    Open { path: String, decoding: Decoding },
    Read { source: String, start: u64, count: u64 },
    Find { source: String, query: String, position: Position, direction: Direction },
    Changed { source: String, version: u64 },
    Close { source: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Result {
    Opened {
        source: String,
        path: String,
        extent: Extent,
    },
    Lines {
        start: u64,
        lines: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        notes: Vec<LineNote>,
    },
    Found {
        found: Option<Match>,
    },
    Changed {
        extent: Extent,
        first_changed_line: u64,
    },
    Closed,
    ReadComplete {
        start: u64,
        count: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub line: u64,
    pub text: String,
    /// Metadata-only reads carry the decoded length without repeating text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utf16_length: Option<u64>,
    pub ended: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<crate::run_facts::OutputNote>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineNote {
    pub line: u64,
    /// UTF-16 offsets in the returned logical row, end exclusive.
    pub from: u64,
    pub to: u64,
    pub note: crate::run_facts::OutputNote,
}

/// The projected viewport returned to the app; text stays canonical.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedLines {
    pub start: u64,
    pub lines: Vec<String>,
    pub notes: Vec<LineNote>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRequest {
    pub source: String,
    pub start: u64,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub machine: MachineName,
    pub result: Result,
}
