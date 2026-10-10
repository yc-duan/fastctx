//! The local uninstaller's preview and line-by-line progress contract.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub what: String,
    #[serde(rename = "where")]
    pub location: Option<String>,
    pub admin: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hub {
    pub name: String,
    pub dir: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Other {
    pub path: String,
    pub version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub everything: bool,
    pub steps: Vec<Item>,
    pub keeps: Vec<Item>,
    pub hub: Option<Hub>,
    pub other: Option<Other>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Left {
    pub kind: String,
    pub what: String,
    #[serde(rename = "where")]
    pub location: Option<String>,
    pub why: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Result {
    Done,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Event {
    Step { step: String, result: Result, why: Option<String> },
    Finished { left: Vec<Left> },
}
