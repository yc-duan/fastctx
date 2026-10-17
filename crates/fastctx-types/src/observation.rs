//! The provenance of answers collected through several accounts and hubs.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::access::{HubId, Written};
use crate::{AccountName, MachineName};

/// A machine in one network. Names describe it; only `(hub, key)` identifies it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineSource {
    pub hub: HubId,
    pub key: String,
    pub written: Written,
}

impl MachineSource {
    pub fn same_machine(&self, other: &Self) -> bool {
        self.hub == other.hub && self.key == other.key
    }
}

/// An account whose part of a machine's answer was not obtained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountFailure {
    pub account: AccountName,
    #[serde(flatten)]
    pub failure: Unanswered,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Unanswered {
    Offline { since: Option<Timestamp> },
    NoAnswer { reason: String },
}

impl std::fmt::Display for AccountFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "as {}: ", self.account)?;
        match &self.failure {
            Unanswered::Offline { since: Some(at) } => write!(f, "offline since {at}"),
            Unanswered::Offline { since: None } => f.write_str("offline"),
            Unanswered::NoAnswer { reason } => f.write_str(reason),
        }
    }
}

/// Additive metadata: absent on answers from older versions or local records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub source: MachineSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unanswered: Vec<AccountFailure>,
}

pub fn written(machine: &MachineName, source: Option<&MachineSource>) -> Written {
    source.map_or_else(|| machine.into(), |source| Written { name: machine.clone(), hub: source.written.hub.clone() })
}

/// Addresses are scoped to a hub, even when a job runs on another machine.
pub fn same_network(one: Option<&MachineSource>, other: Option<&MachineSource>) -> bool {
    match (one, other) {
        (Some(one), Some(other)) => one.hub == other.hub,
        (None, None) => true,
        _ => false,
    }
}

pub fn union<T: PartialEq>(into: &mut Vec<T>, more: impl IntoIterator<Item = T>) {
    for item in more {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}
