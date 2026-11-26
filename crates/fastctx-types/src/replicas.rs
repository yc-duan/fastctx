//! Metadata-only discovery of the current documents a computer must keep.

use crate::{AccountName, Address, TownName, access::TownId, history::HistoryRevision, owned_records::Conversation};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    Project { town: TownId, name: TownName },
    Conversation { conversation: Conversation },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Document {
    Plan { owner: Address },
    History { scope: Scope, name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub owner: AccountName,
    pub opened: Timestamp,
    pub document: Document,
    pub version: Option<u64>,
    pub retired: bool,
    /// A history document deleted for good, retired as well: its copy is
    /// withdrawn as a retired one's, and its file written again starts the
    /// document anew.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
    pub history: Option<HistoryRevision>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    /// Empty histories also have folders, so the first document can be written.
    pub scopes: Vec<Scope>,
    pub documents: Vec<Head>,
}
