//! File-name completion for a conversation. Searches live on the machine
//! holding the files; they never read file contents or expand the scope.

use serde::{Deserialize, Serialize};

use crate::{MachineName, ResidentId};

/// Legacy `Start` searches expire this long after creation or their last
/// accepted query. Connection-owned `Open` searches do not expire while idle.
pub const START_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "scope")]
pub enum FileSearchScope {
    /// The resident's current project address book, restricted to its machine.
    /// `roots` is used only when that resident is outside every project.
    Resident { resident: ResidentId, roots: Vec<String> },
    /// Explicit absolute paths, for a conversation not yet opened.
    Roots { roots: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum FileSearchRequest {
    /// Start when opening the conversation's composer, before the first `@`.
    /// The receiver owns the index until Close or five minutes without a query,
    /// including when the caller leaves before receiving its ID.
    Start { scope: FileSearchScope },
    /// Own an index through this connection. Closing the connection releases it.
    /// Interfaces use this form; Start remains available for legacy callers.
    Open { scope: FileSearchScope },
    /// Ask on each edit, and again while `ready` is false. Queries are literal
    /// fuzzy text (including spaces and punctuation), not a query language.
    Query { search: String, query: String, limit: usize },
    /// Release the index and file-system watches when leaving the composer.
    Close { search: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchReply {
    pub machine: MachineName,
    #[serde(flatten)]
    pub result: FileSearchResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum FileSearchResult {
    Started {
        search: String,
    },
    Matches {
        /// Echoed verbatim so an interface can discard replies to older edits.
        query: String,
        /// False while scanning or refreshing. An existing snapshot may
        /// still supply revalidated hits; keep querying for newer names.
        ready: bool,
        /// Matching names in the searched snapshot, before `limit`.
        total: usize,
        files: Vec<FileMatch>,
    },
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMatch {
    /// Absolute native path to insert into the message. No escaping or `@`
    /// is added: the interface owns text insertion, the agent owns reading.
    pub path: String,
    /// The authorized root and path within it, for disambiguating labels.
    pub root: String,
    pub relative: String,
}
