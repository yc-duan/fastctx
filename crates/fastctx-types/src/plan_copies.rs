//! Revision-aware exchange for a plan's working copy. Only hubs advertising
//! `plan_copies` accept these requests; ordinary plan readers remain unchanged.

use crate::{Address, plan::Page};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    /// A missing historical revision is an error, never an empty base.
    Revision { owner: Address, version: u64 },
    /// Merge a file based on `base`; concurrent conflicting lines favor the
    /// file while the replaced hub version remains in history.
    Merge { owner: Address, base: Option<u64>, markdown: String },
    /// Confirm an exact body before retiring an obsolete local file.
    Contains { owner: Address, markdown: String },
}

impl Request {
    pub fn owner(&self) -> &Address {
        match self {
            Self::Revision { owner, .. } | Self::Merge { owner, .. } | Self::Contains { owner, .. } => owner,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Merge { markdown, .. } | Self::Contains { markdown, .. } => crate::plan::validate(markdown),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Reply {
    Page { page: Box<Page> },
    Contains { found: bool },
}
