//! The graphical shell's connection choice and device status. These are
//! application commands, not new IPC or network protocol frames.

use serde::{Deserialize, Serialize};

use crate::MachineName;
use crate::world::LinkState;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionMode {
    Local,
    Device,
}

/// Available before logging in, while offline, and when no machine can
/// answer. Contains no key or token material.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceState {
    /// This device's name, the same on every hub; `None` before its first
    /// login.
    pub name: Option<MachineName>,
    pub hubs: Vec<DeviceHub>,
}

/// One hub this device is logged in to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHub {
    pub hub: crate::HubName,
    pub link: LinkState,
    pub logins: Vec<crate::AccountName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionState {
    pub mode: ConnectionMode,
    pub device: Option<DeviceState>,
}

/// A device's login does not install or start a node or a system service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceJoined {
    pub hub: crate::HubName,
    pub name: MachineName,
    pub account: crate::AccountName,
}
