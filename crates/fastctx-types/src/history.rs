//! Versioned topic documents belonging to a project or a conversation.

pub mod markdown;
pub mod stored;
pub mod words;

use crate::{MachineName, ResidentId, TownName, project::Stamp, residents::Host};
use serde::{Deserialize, Serialize};

pub const MAX_DOCUMENT_BYTES: usize = 512 * 1024;
/// A description is one line in the list every conversation holds, all of
/// it, so each is kept short (`world-history.md`, 为 grep 写).
pub const MAX_DESCRIPTION_BYTES: usize = 1024;
/// Each history holds at most this many documents, including retired ones.
pub const MAX_DOCUMENTS: usize = 2048;
pub const RECORD_KIND: &str = "history";

/// The public scope uses project names and conversation addresses. Resolution
/// supplies the account and stable identity before reading or changing records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    Project { project: TownName },
    Conversation { resident: ResidentId, machine: MachineName },
}

/// Attribution comes from the authenticated caller, never the Markdown header.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    #[serde(flatten)]
    pub stamp: Stamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Host>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRevision {
    pub name: String,
    pub description: String,
    pub version: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retired: bool,
    pub author: Author,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    /// Latest revisions, including retired documents, newest first.
    pub documents: Vec<HistoryRevision>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub revision: HistoryRevision,
    pub text: String,
    /// All revisions, oldest first, including retirement and restoration.
    pub versions: Vec<HistoryRevision>,
}

impl Catalog {
    pub fn sort(&mut self) {
        self.documents.sort_by(|a, b| b.author.stamp.at.cmp(&a.author.stamp.at).then(a.name.cmp(&b.name)));
    }

    pub fn for_reader(&mut self, usable: &dyn Fn(&MachineName) -> bool) {
        for revision in &mut self.documents {
            revision.author.stamp.for_reader(usable);
        }
    }
}

impl Document {
    pub fn for_reader(&mut self, usable: &dyn Fn(&MachineName) -> bool) {
        self.revision.author.stamp.for_reader(usable);
        for revision in &mut self.versions {
            revision.author.stamp.for_reader(usable);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum HistoryCall {
    List {
        scope: Scope,
    },
    Read {
        scope: Scope,
        name: String,
        version: Option<u64>,
    },
    /// None creates a new name. Some(n) requires exactly the displayed version.
    Save {
        scope: Scope,
        name: String,
        text: String,
        expected: Option<u64>,
    },
    Retire {
        scope: Scope,
        name: String,
        expected: u64,
    },
    /// Restoring copies the chosen version into a new, live revision.
    Restore {
        scope: Scope,
        name: String,
        version: u64,
    },
}

impl HistoryCall {
    pub fn scope(&self) -> &Scope {
        match self {
            Self::List { scope }
            | Self::Read { scope, .. }
            | Self::Save { scope, .. }
            | Self::Retire { scope, .. }
            | Self::Restore { scope, .. } => scope,
        }
    }

    pub fn writes(&self) -> bool {
        matches!(self, Self::Save { .. } | Self::Retire { .. } | Self::Restore { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryReply {
    Catalog { catalog: Catalog },
    Document { document: Box<Document> },
    Saved { revision: Box<HistoryRevision> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Watched {
    Catalog { catalog: Catalog },
}

pub fn conflict(current: Option<u64>) -> String {
    match current {
        Some(version) => format!("the document changed; its latest version is {version}; read it before saving again"),
        None => "the document has no version; read the history before saving again".into(),
    }
}
