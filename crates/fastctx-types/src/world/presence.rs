//! A machine's visibility, decided by the local reader of each hub's directory.

use serde::{Deserialize, Serialize};

/// Why a machine is, or is not, available to this device's logged-in accounts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineAccess {
    /// The last directory grants at least one login permission to look.
    Granted,
    /// A directory received on the current, online connection grants no login permission to look.
    NotGranted,
    /// The directory needed to decide has not arrived on this connection.
    #[default]
    Unknown,
    /// This device's link ended; this says nothing about another machine's grants.
    Stopped,
}

impl crate::tolerant::Tolerant for MachineAccess {
    const ALL: &'static [Self] = &[Self::Granted, Self::NotGranted, Self::Unknown, Self::Stopped];
    const UNKNOWN: Self = Self::Unknown;
}

/// One hub's decision, retaining the revision of the directory it used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubMachinePresence {
    pub hub: crate::access::HubId,
    pub revision: u64,
    #[serde(default, deserialize_with = "crate::tolerant::read")]
    pub access: MachineAccess,
}

/// Visibility of a currently listed or previously known machine, named as the reader writes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachinePresence {
    pub machine: String,
    pub hubs: Vec<HubMachinePresence>,
}
