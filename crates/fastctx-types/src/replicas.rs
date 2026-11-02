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
    pub history: Option<HistoryRevision>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    /// Empty histories also have folders, so the first document can be written.
    pub scopes: Vec<Scope>,
    pub documents: Vec<Head>,
}
