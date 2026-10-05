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

/// Available before joining, while offline, and when no machine can answer.
/// Contains no key or invitation material.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceState {
    pub name: Option<MachineName>,
    pub world: Option<String>,
    pub link: LinkState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionState {
    pub mode: ConnectionMode,
    pub device: Option<DeviceState>,
}

/// Device admission does not install or start a node or a system service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceJoined {
    pub world: String,
    pub name: MachineName,
}
