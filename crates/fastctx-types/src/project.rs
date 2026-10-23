//! A town's shared ledger as files: the versions of its record (立项属性) and
//! its journals (`world-projects.md`, `world-journals.md`, `world-map.md`).
//!
//! ```text
//! project/v000001.json                               the town's record, one file a version
//! journals/<journal>/description/v000001.json        a journal's description, one file a version
//! journals/<journal>/entries/<entry>/entry.json      an entry's stamp
//! journals/<journal>/entries/<entry>/text.md         its text
//! journals/<journal>/entries/<entry>/attachments/…   the attachments it carries
//! ```
//!
//! Each version or entry is one entry of the ledger, so the latest version
//! of anything is the one with the highest number, and two writers of the
//! same version cannot both succeed.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::residents::Host;
use crate::{Address, AgentName, EntryId, JournalName, MachineName, TownName};

/// Who wrote something into a town's ledger, as the World saw it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Writer {
    Page {
        page: Box<crate::pages::PageSource>,
    },
    /// The user, at a terminal of one of their machines.
    User,
    Resident {
        address: Address,
    },
    /// An agent's session the World has no resident for, from what its host
    /// put in the environment of the command it ran.
    Session {
        agent: AgentName,
    },
}

/// One version of a town's record: what a resident coming in must know.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub version: u32,
    /// The project's name, in any words.
    pub name: String,
    /// What it is for, in a sentence or two.
    pub what: String,
    /// The machines the town spans.
    pub machines: Vec<MachineName>,
    /// Where things are: an address book, not a boundary.
    pub places: Vec<Listing>,
    /// Where the town's canon is: a directory of documents.
    pub canon: Option<Location>,
    pub by: Writer,
    pub on: MachineName,
    pub at: Timestamp,
}

/// A path on a machine, written the way that machine writes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub machine: MachineName,
    pub path: String,
}

/// One line of a town's address book.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub what: String,
    #[serde(flatten)]
    pub location: Location,
}

/// Who wrote something, where and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub by: Writer,
    pub on: MachineName,
    pub at: Timestamp,
}

/// What a version of a town's record says, before the World stamps it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordFields {
    pub name: Option<String>,
    pub what: Option<String>,
    pub machines: Option<Vec<MachineName>>,
    /// The whole address book, replacing the one before.
    pub places: Option<Vec<Listing>>,
    pub canon: Option<Location>,
}

/// One version of a journal's description: what the journal covers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Description {
    pub version: u32,
    pub text: String,
    pub by: Writer,
    pub on: MachineName,
    pub at: Timestamp,
}

/// The World's stamp on a journal entry, beside its text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryStamp {
    pub id: EntryId,
    pub at: Timestamp,
    pub machine: MachineName,
    pub by: Writer,
    /// The agent program and version, when the writer is a resident the
    /// World knows.
    pub agent: Option<Host>,
    pub model: Option<Model>,
    /// The session or thread on its host, when known.
    pub session: Option<String>,
    /// Earlier entries this one answers.
    pub cites: Vec<EntryId>,
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    /// The resident said so; its host did not tell the World.
    pub self_reported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub kind: AttachmentKind,
    pub bytes: u64,
    pub sha256: String,
    pub kept: Kept,
}

/// What an attachment's bytes are, as the World read them when it took it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    /// UTF-8 text.
    Text,
    Image,
    Pdf,
    Binary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Kept {
    /// In the entry's `attachments` directory, on every machine.
    InEntry,
    /// Too large for the ledger: a copy kept on the machine that wrote the
    /// entry.
    OnMachine { location: Location },
}

/// A town as `fastctx project list` shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TownSummary {
    pub town: TownName,
    pub record: Record,
    pub journals: usize,
}

/// One journal as the map and `fastctx journal list` show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalSummary {
    pub journal: JournalName,
    pub description: Option<Description>,
    pub entries: usize,
    /// How many residents, the user among them, wrote its entries.
    pub writers: usize,
    pub latest: Option<EntryStamp>,
}

/// One document of a town's canon, as the map shows it: its title and the
/// paragraph that says what it covers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonHead {
    /// The file's name in the canon's directory.
    pub file: String,
    pub title: String,
    pub scope: String,
}

/// What a town's map is made of (`world-map.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapView {
    /// Generated project-memory block. Absent when the account has disabled it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    pub town: TownName,
    /// The town's ledger on the machine that made the map.
    pub ledger: String,
    pub record: Record,
    /// Who opened the town: the writer of its first version.
    pub opened: Stamp,
    /// Most recently added to first.
    pub journals: Vec<JournalSummary>,
    pub pages: Vec<crate::pages::PageSummary>,
    /// The canon's documents, or why they could not be read.
    pub canon: Option<Result<Vec<CanonHead>, String>>,
}

/// One entry as it is read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryView {
    pub stamp: EntryStamp,
    pub text: String,
    /// Later entries that cite this one.
    pub mentioned_by: Vec<EntryId>,
}

/// A part of a journal as `fastctx journal read` gives it: the latest
/// entries that fit, in order, before the one it was asked to read before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalView {
    pub town: TownName,
    pub journal: JournalName,
    /// The journal's directory on the machine that read it.
    pub directory: String,
    pub description: Option<Description>,
    /// Its position among all entries, from 1.
    pub first: usize,
    pub entries: Vec<EntryView>,
    pub total: usize,
    pub writers: usize,
}

pub fn record_path(version: u32) -> String {
    format!("project/v{version:06}.json")
}

pub fn description_path(journal: &JournalName, version: u32) -> String {
    format!("journals/{journal}/description/v{version:06}.json")
}

/// The directory of an entry, relative to the ledger.
pub fn entry_path(journal: &JournalName, id: &EntryId) -> String {
    format!("journals/{journal}/entries/{id}")
}

/// The version a file such as `v000003.json` holds.
pub fn version_of(file_name: &str) -> Option<u32> {
    file_name.strip_prefix('v')?.strip_suffix(".json")?.parse().ok()
}
