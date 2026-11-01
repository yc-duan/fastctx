//! What went wrong with a machine of a network, or with the network itself,
//! as its hub tells those who should hear of it (`specs/world-engine.md`, 量与
//! 告警): a machine's to the accounts that hold it, the network's to its super
//! administrator. A device shows each once, however many devices of the
//! account see it.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::MachineName;
use crate::health::Near;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trouble {
    /// Counts the hub's troubles; a device tells each (hub, id) once.
    pub id: u64,
    pub at: Timestamp,
    /// The machine it is of; none for the network as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<MachineName>,
    #[serde(deserialize_with = "crate::tolerant::read")]
    pub kind: TroubleKind,
    /// How many: the times the machine connected again within the hour, or
    /// the machines that lost their line together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// How long, in seconds: what has been stuck, or nothing was heard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
    /// The bound that came near its limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near: Option<Near>,
    /// The version a stopped rollout was of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TroubleKind {
    /// A machine meant to stay up stopped answering its hub.
    Silent,
    /// The machine connected again more than three times within an hour.
    Drops,
    /// Some of the machine's work has not gone on for a minute.
    Stuck,
    /// One of the machine's bounds came near its limit.
    Near,
    /// The machine went on breaking the rules of its threads.
    Violations,
    /// Many of the network's machines lost their line together.
    Outage,
    /// The hub's own work is held up or stuck.
    Hub,
    /// The hub stopped a version going out: the machines that took it fared
    /// worse than the others (`crate::update::Stop`).
    Rollout,
    /// One a later release tells.
    Other,
}

impl crate::tolerant::Tolerant for TroubleKind {
    const ALL: &'static [Self] = &[
        TroubleKind::Silent,
        TroubleKind::Drops,
        TroubleKind::Stuck,
        TroubleKind::Near,
        TroubleKind::Violations,
        TroubleKind::Outage,
        TroubleKind::Hub,
        TroubleKind::Rollout,
        TroubleKind::Other,
    ];
    const UNKNOWN: Self = TroubleKind::Other;
}
