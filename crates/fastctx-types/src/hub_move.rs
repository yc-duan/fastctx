//! Moving a network's hub to another machine (`world-network.md`, 把中枢搬到别处):
//! what the super administrator's interfaces ask of it and are told.
//!
//! The hub's identity is the few things in its directory, so the whole
//! directory goes: every machine keeps the certificate it pinned and its
//! login, and only learns the new address. The move goes step by step,
//! each one said as it begins (`MoveStage`); until the old hub hands over,
//! it keeps serving and the move can be called off without anything lost.

use std::net::{IpAddr, SocketAddr};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::world::Form;
use crate::{AccountName, MachineName};

/// What the super administrator asks of a move.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "request")]
pub enum HubMoveRequest {
    /// The account's own machines, each with whether it can take the hub
    /// now and the addresses others might reach it at.
    Targets,
    /// Moves the hub to `to`, reached at `addresses` from then on.
    Start { to: MachineName, addresses: Vec<SocketAddr> },
    /// The move under way, and the earlier moves of this hub.
    Status,
    /// Tries the step the move waits at again now.
    Retry,
    /// Calls off the move under way, which is possible until the old hub
    /// hands over.
    Cancel,
    /// Stops the old place forwarding to the new one before every machine
    /// followed; those that had not must log in again.
    StopForwarding { id: String },
    /// Deletes the data the old place kept, disabled, after the move.
    DeleteOld { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HubMoveReply {
    Targets {
        machines: Vec<MoveTarget>,
        /// The port the hub listens on now, which the new place is offered
        /// by default.
        port: Option<u16>,
    },
    /// The newest first.
    Moves { moves: Vec<HubMove> },
}

/// One of the super administrator's machines, as a place for the hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveTarget {
    pub name: MachineName,
    pub online: bool,
    /// `windows` or `linux`, when it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<Form>,
    /// It goes to sleep on its own, as a laptop does: a hub there is away
    /// whenever it sleeps.
    pub sleeps: bool,
    /// Its own network addresses, physical ones with a way out first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<InterfaceAddress>,
    /// The address the hub sees its connection come from: its public
    /// address when it is behind a router, or the hub's own machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<IpAddr>,
    /// The hub runs on it now.
    pub hosts_hub: bool,
    /// Why it cannot take the hub now; `None` when it can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// An address of a machine's network card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceAddress {
    pub interface: String,
    pub address: IpAddr,
    /// Ethernet or Wi-Fi with a way out, not a TUN, VPN or virtual switch.
    pub physical: bool,
}

/// One move of the hub, as it stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubMove {
    pub id: String,
    /// The machine the hub moves to.
    pub to: MachineName,
    /// Where it is reached there.
    pub addresses: Vec<SocketAddr>,
    /// Where it was reached before, where the old place forwards from.
    pub from: Vec<SocketAddr>,
    pub by: AccountName,
    pub started: Timestamp,
    pub stage: MoveStage,
    /// Why the move does not get past `stage`, as last tried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What waits for someone at the new place, while the move does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<Waiting>,
    /// When the new place took over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switched: Option<Timestamp>,
    /// After the switch: who has not followed yet, and the old place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub following: Option<Following>,
}

/// The steps of a move, in order. Each is the step under way; the ones
/// before it are done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveStage {
    /// The new place takes the hub's identity and makes its directory.
    Preparing,
    /// The hub's system service is to be installed at the new place, which
    /// takes administrator rights there (`Waiting`).
    Service,
    /// The old hub reaches the new one at its address.
    Reaching,
    /// The records go to the new place while the old hub still serves.
    Copying,
    /// The old hub stops, the last records go, and the new place takes over.
    Switching,
    /// The new place serves; the old one forwards to it until every machine
    /// follows.
    Following,
    /// Every machine followed, or the forwarding was stopped.
    Done,
    /// Called off before the switch; the old hub serves on.
    Cancelled,
}

/// What the new place waits for someone there to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Waiting {
    /// The hub's system service, installed with administrator rights on
    /// that machine: `fastctx hub arrive` there does it.
    Service {
        /// The new hub's directory there, as that machine writes it.
        dir: String,
        /// That machine's system.
        os: String,
    },
}

/// The machines and devices that have not followed to the new place, and
/// how the old place stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Following {
    /// Machines that have not connected to the new place but through the
    /// old one since the switch.
    pub machines: Vec<MachineName>,
    /// Phones and other devices, likewise.
    pub devices: Vec<MachineName>,
    pub old: OldPlace,
    /// Whether the old place is connected to the new one now, so it is
    /// told at once what it is asked.
    pub old_connected: bool,
}

/// The old place after the switch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum OldPlace {
    /// It forwards every connection to the new place.
    Forwarding,
    /// It stopped forwarding: when every machine had followed, or `early`,
    /// as the super administrator asked. Its data is kept, disabled.
    Stopped { at: Timestamp, early: bool },
    /// Asked to delete its data; it does when it is connected.
    Deleting { at: Timestamp },
    /// Its data is deleted.
    Deleted { at: Timestamp },
}
