//! Account-scoped update management, negotiated with `Network::updates_view`.

use super::{App, Channel, CheckFailure, Due, Kind, Manifest, Policy, Stage};
use crate::{AccountName, MachineName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionReason {
    NotOwner,
    Visitor,
    Phone,
    TooOld,
    Development,
    Offline,
    UnknownVersion,
    MissingMachine,
    NoPackage,
    NotNewer,
    Unavailable,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permission {
    pub allowed: bool,
    pub reason: Option<ActionReason>,
}

impl Permission {
    pub fn yes() -> Self {
        Self { allowed: true, reason: None }
    }
    pub fn no(reason: ActionReason) -> Self {
        Self { allowed: false, reason: Some(reason) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actions {
    pub update: Permission,
    pub choose: Permission,
    pub settings: Permission,
    pub schedule: Permission,
    pub cancel: Permission,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<Permission>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineUpdate {
    pub machine: MachineName,
    pub owner: AccountName,
    pub group: Option<String>,
    pub kind: Kind,
    pub online: bool,
    pub reported: Option<Timestamp>,
    pub kernel: Option<String>,
    pub app: Option<App>,
    pub target: Option<String>,
    pub channel: Channel,
    pub auto: Option<bool>,
    pub auto_chosen: bool,
    pub relays: Option<bool>,
    pub github: Option<bool>,
    pub github_at: Option<Timestamp>,
    pub check_failure: Option<CheckFailure>,
    pub checking: bool,
    pub stage: Stage,
    pub due: Option<Due>,
    pub offer: Option<Manifest>,
    pub actions: Actions,
    #[serde(default)]
    pub repair_supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_due: Option<super::RepairDue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<crate::repair::Report>,

}

/// A known version, including a listed release whose manifest is unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionChoice {
    pub version: String,
    pub published: Option<String>,
    pub summary: Option<String>,
    pub manifest: Option<Manifest>,
    pub problem: Option<super::Problem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub hub: String,
    pub stable: Option<Manifest>,
    pub beta: Option<Manifest>,
    pub releases: Vec<Manifest>,
    #[serde(default)]
    pub versions: Vec<VersionChoice>,
    /// Only the super administrator receives the network's editable policy.
    pub policy: Option<Policy>,
    pub machines: Vec<MachineUpdate>,
    pub available: usize,
    #[serde(default)]
    pub repair_supported: bool,

}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum ViewCall {
    Repair { machines: Vec<MachineName>, #[serde(default)] version: Option<String> },
    CancelRepair { machines: Vec<MachineName> },
    /// The hub's node reports a legacy machine's independently read receipt.
    RepairResult { machine: MachineName, result: Box<crate::repair::Report> },
    View,
    /// Reads one listed manifest on demand; ordinary view polling never does IO.
    Release {
        version: String,
    },
    /// None withdraws the current arrangement for each named machine.
    Schedule {
        machines: Vec<MachineName>,
        version: Option<String>,
    },
    Policy {
        minimum: Option<String>,
        managed_auto: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scheduled {
    pub machine: MachineName,
    pub done: bool,
    pub reason: Option<ActionReason>,
    pub why: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewReply {
    Release { choice: Box<VersionChoice> },
    View { view: Box<View> },
    Scheduled { results: Vec<Scheduled> },
    Done,
}
