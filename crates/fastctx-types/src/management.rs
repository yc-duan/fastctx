//! User actions on a resident's membership, presence and project.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::TownName;
use crate::residents::{Activity, Landed};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum Management {
    Remove,
    Sleep,
    Wake,
    /// None leaves every project.
    Move {
        town: Option<TownName>,
    },
    /// The machines and folders the user points it to, replacing those
    /// before (`world-gui.md`, 对话).
    Focus {
        focus: crate::focus::Focus,
    },
}

/// Each completed stage remains reported if a later stage fails. A host
/// refusal never implies that the World closed a user's own window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Managed {
    /// Membership is removed even if the host step failed. Activity below
    /// remains the last observation of the host, not a membership flag.
    pub removed: Option<Timestamp>,
    pub town: Option<TownName>,
    pub activity: Option<Activity>,
    pub steps: Vec<ManagementStep>,
    pub notification: Option<Landed>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagementStep {
    /// `host`, `records`, `tools`, or `notification`.
    pub stage: String,
    pub outcome: ManagementOutcome,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagementOutcome {
    Done,
    Pending,
    Unsupported,
    Failed,
}
