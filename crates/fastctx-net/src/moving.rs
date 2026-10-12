//! What moving the hub adds to the wire (`fastctx_types::hub_move`).
//!
//! The super administrator's device asks the hub (`wire::Question::HubMove`).
//! The machine the hub moves to sees the move in its view, takes the hub's
//! identity from it and says how it stands (`wire::Question::HubArrival`).
//! The old hub then dials the new one at its address and proves it holds
//! the move's own key (`wire::Intent::Handover`), which the new place got
//! with the identity: over that connection the records go across and the
//! new hub takes over, and later the old place, forwarding, hears how many
//! machines still come through it.

use std::net::SocketAddr;

use fastctx_types::HubName;
use serde::{Deserialize, Serialize};

use crate::ids::HubId;
use crate::sign::{Bytes, PublicKey};

/// What the machine a hub moves to asks of the hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "request")]
pub enum ArrivalRequest {
    /// The hub's identity, for the move `id` to the asking machine.
    Take { id: String },
    /// How the new place stands in the move `id`.
    Report { id: String, report: ArrivalReport },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ArrivalReport {
    /// The new hub's directory is made, at `dir` on a machine running `os`;
    /// its system service is still to be installed there.
    Prepared { dir: String, os: String },
    /// The hub's system service is installed there.
    Installed,
    /// The directory could not be made, and why.
    Failed { reason: String },
    /// Nothing of the move, called off, is here any more.
    Gone,
}

/// The hub's identity, for the machine it moves to: everything a hub is
/// known by, and what it needs to take the old hub's records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub id: HubId,
    pub name: HubName,
    pub self_registration: bool,
    /// The certificate every member pinned, and its key, as DER.
    pub certificate: Bytes,
    pub key: Bytes,
    /// The hub's login secrets (`crate::login::HubSecrets`).
    pub secrets: Bytes,
    /// The public half of the move's own key, which the old hub proves
    /// itself with when it hands its records over.
    pub mover: PublicKey,
    /// Where the hub is reached at the new place.
    pub addresses: Vec<SocketAddr>,
    /// Where it was reached until now.
    pub from: Vec<SocketAddr>,
}

/// What the old hub dials the new one for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoverRole {
    /// To copy the records across and hand over.
    Sync,
    /// As the old place after the switch: to hear how many machines still
    /// come through it, and to be told to stop and to delete its data.
    Old,
}
