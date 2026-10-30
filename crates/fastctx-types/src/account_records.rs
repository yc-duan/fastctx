//! Account-owned JSON records. Revisions survive deletion, preventing stale writes
//! from succeeding after a key is deleted and created again.

pub mod words;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AccountName, MachineName};

pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const MAX_ACCOUNT_BYTES: usize = 16 * 1024 * 1024;
/// Live records of one kind. Tombstones still count toward the account byte limit.
pub const MAX_KIND_KEYS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub kind: String,
    pub key: String,
    pub version: u64,
    pub by: MachineName,
    pub at: Timestamp,
    pub deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub metadata: Metadata,
    /// A deleted record has a null body. A live JSON null is distinct by metadata.
    pub body: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    Read {
        kind: String,
        key: String,
    },
    List {
        kind: String,
    },
    /// `expected == None` requires a key with no revision. A deleted key still
    /// has a revision, which must be supplied explicitly to recreate it.
    Write {
        kind: String,
        key: String,
        expected: Option<u64>,
        body: Value,
    },
    Delete {
        kind: String,
        key: String,
        /// None records a tombstone only if this key has never been written.
        expected: Option<u64>,
    },
}

impl Request {
    pub fn kind(&self) -> &str {
        match self {
            Self::Read { kind, .. } | Self::List { kind } | Self::Write { kind, .. } | Self::Delete { kind, .. } => {
                kind
            }
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_kind(self.kind())?;
        let key = match self {
            Self::Read { key, .. } | Self::Write { key, .. } | Self::Delete { key, .. } => key,
            Self::List { .. } => return Ok(()),
        };
        if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
            return Err(words::KEY.into());
        }
        if let Self::Write { body, .. } = self {
            let size = serde_json::to_vec(body).map_err(|error| error.to_string())?.len();
            let limit = if self.kind() == crate::skills::KIND || self.kind() == crate::skills::INDEX_KIND {
                crate::skill_files::ENCODED_LIMIT
            } else {
                MAX_BODY_BYTES
            };
            if size > limit {
                return Err(words::body_limit(size));
            }
        }
        Ok(())
    }
}

pub fn validate_kind(kind: &str) -> Result<(), String> {
    if kind.is_empty()
        || kind.len() > 32
        || !kind.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
    {
        return Err(words::KIND.into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Read {
        record: Option<Record>,
    },
    Listed {
        entries: Vec<Metadata>,
    },
    Written {
        metadata: Metadata,
    },
    /// No body is returned: the caller must read before choosing another write.
    Conflict {
        current: Option<Metadata>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Changed {
    pub account: AccountName,
    pub metadata: Metadata,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Watched {
    /// A fresh authoritative listing, including after a reconnect or missed hints.
    Current {
        entries: Vec<Metadata>,
    },
    Changed {
        metadata: Metadata,
    },
}
