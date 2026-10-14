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

/// The network this computer is a managed machine on, and its owner there,
/// while the owner has not taken it off: only an administrator of this
/// computer takes Fastctx off it then (`world-network.md`, 受管机器).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Managed {
    pub hub: String,
    pub owner: String,
}

impl Managed {
    /// Why an uninstall that is not an administrator's stops here.
    pub fn refusal(&self) -> String {
        format!(
            "this computer is a managed machine of {} on {}: its owner takes it off the network in Fastctx, and Fastctx can be removed here after; an administrator of this computer may remove it now",
            self.owner, self.hub
        )
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed: Option<Managed>,
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
