//! Observations shared by file viewing and writing. Hashes always cover disk bytes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// The evidence captured by one view, without reading unseen bytes just to hash them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeenVersion {
    Full { sha256: String },
    Partial { size: u64, modified: SystemTime },
}

/// A view's encoding choice applies only to the revision it observed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SeenFile {
    pub version: SeenVersion,
    /// An explicit codec selection from view, never an automatically inferred label.
    pub encoding: Option<String>,
}

/// Latest observation per canonical absolute path, on this machine and for this resident.
/// The caller loads these from `seen`; the writing core does not invent observations.
pub type SeenFiles = BTreeMap<PathBuf, SeenFile>;
