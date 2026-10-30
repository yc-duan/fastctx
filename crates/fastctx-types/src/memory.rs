//! Project memory: immutable journals, versioned specs, and their catalog.

pub mod markdown;

pub mod words;
pub use words::LEGACY;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::TownName;
use crate::project::{Location, Stamp};
use crate::residents::Host;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Journal,
    Spec,
}

impl Kind {
    pub fn folder(self) -> &'static str {
        match self {
            Self::Journal => "journals",
            Self::Spec => "specs",
        }
    }
}

/// Attribution is supplied by the World, never taken from Markdown headers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    #[serde(flatten)]
    pub stamp: Stamp,
    pub host: Option<Host>,
    pub model: Option<String>,
    pub session: Option<String>,
}

impl Author {
    /// As told to someone who may work only on the machines `usable`
    /// admits (`Stamp::for_reader`): the host's session is not named either
    /// where the writer is not.
    pub fn for_reader(&mut self, usable: &dyn Fn(&crate::MachineName) -> bool) {
        self.stamp.for_reader(usable);
        if self.stamp.on.is_none() || matches!(self.stamp.by, crate::project::Writer::Elsewhere { .. }) {
            self.session = None;
        }
    }
}

impl Catalog {
    /// As told to someone who may work only on the machines `usable` admits.
    pub fn for_reader(&mut self, usable: &dyn Fn(&crate::MachineName) -> bool) {
        for revision in self.journals.iter_mut().chain(self.specs.iter_mut()) {
            revision.author.for_reader(usable);
        }
    }
}

impl Document {
    /// As told to someone who may work only on the machines `usable` admits:
    /// its authors, a journal's header that names them, and evidence kept on
    /// a machine they may not work on.
    pub fn for_reader(&mut self, usable: &dyn Fn(&crate::MachineName) -> bool) {
        let written = self.revision.author.clone();
        self.revision.author.for_reader(usable);
        if self.revision.kind == Kind::Journal && self.revision.author != written {
            self.text = markdown::attributed(&self.text, &self.revision.author);
        }
        for revision in &mut self.versions {
            revision.author.for_reader(usable);
        }
        for evidence in &mut self.evidence {
            evidence.author.for_reader(usable);
            if evidence.location.as_ref().is_some_and(|location| !usable(&location.machine)) {
                evidence.location = None;
            }
        }
    }
}

/// Metadata of one immutable revision. The text is stored beside it as Markdown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub kind: Kind,
    pub name: String,
    pub description: String,
    pub version: u32,
    pub author: Author,
}

impl Revision {
    pub fn path(&self, extension: &str) -> String {
        format!("memory/{}/v{:06}.{extension}", self.name, self.version)
    }
}

/// Evidence is immutable too. Large files stay on the writer's machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub journal: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub location: Option<Location>,
    pub author: Author,
}

impl Evidence {
    pub fn path(&self) -> String {
        format!("memory/{}/evidence/{}.json", self.journal, self.name)
    }

    pub fn content_path(&self) -> String {
        format!("memory/{}/files/{}", self.journal, self.name)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub journals: Vec<Revision>,
    pub specs: Vec<Revision>,
}

/// Real replica directories, returned by the computer that keeps them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folders {
    pub machine: crate::MachineName,
    pub journals: String,
    pub specs: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectIndex {
    pub seq: u64,
    /// Existing project-page metadata, without the published HTML.
    #[serde(default)]
    pub pages: Vec<crate::pages::PageSummary>,
    pub record: crate::project::Record,
    pub opened: Stamp,
    pub catalog: Catalog,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub revision: Revision,
    pub text: String,
    pub versions: Vec<Revision>,
    pub evidence: Vec<Evidence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum MemoryCall {
    List {
        town: TownName,
    },
    Read {
        town: TownName,
        name: String,
        version: Option<u32>,
    },
    /// None creates a document; Some(n) replaces exactly revision n of a spec.
    Save {
        town: TownName,
        kind: Kind,
        name: String,
        text: String,
        expected: Option<u32>,
    },
}

impl MemoryCall {
    pub fn town(&self) -> &TownName {
        match self {
            Self::List { town } | Self::Read { town, .. } | Self::Save { town, .. } => town,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MemoryReply {
    Catalog { catalog: Catalog },
    Document { document: Box<Document> },
    Saved { revision: Box<Revision> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changed {
    pub town: TownName,
    pub kind: Kind,
    pub name: String,
    pub version: u32,
    pub author: Author,
}

/// The account's switch. An absent configuration is disabled.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub enabled: bool,
}

pub fn date(at: Timestamp) -> String {
    at.strftime("%Y-%m-%d").to_string()
}
