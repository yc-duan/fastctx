//! Account-owned placement metadata contains coordinates, never document bodies.

use crate::{Address, MachineName};
use serde::{Deserialize, Serialize};

pub const KIND: &str = "working-place";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub owner: Address,
    pub machine: Option<MachineName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub machine: MachineName,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resource {
    Directory,
    Plan,
    Skill { name: String, version: u64 },
    Files { key: String, files: Vec<crate::skill_files::File> },
}
