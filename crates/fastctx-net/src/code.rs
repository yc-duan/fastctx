//! The codes a person carries from one machine to another to join a World:
//! the hub's founding code for the first machine, and an invitation for
//! every machine after it.
//!
//! A code is everything a machine needs to trust the World before it has
//! talked to anyone: where the hub is, the hub's certificate, and the
//! founder's key; it travels with the person, not through the hub. It is a
//! secret until it is used, since whoever holds it can join.

use std::net::SocketAddr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::sign::{PublicKey, Secret, Signed};
use crate::trust::{CertPin, WorldId};

const PREFIX: &str = "fxw3.";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Code {
    /// Printed by a new hub: the first machine to join with it founds the
    /// World.
    Founding { hub: Vec<SocketAddr>, cert: CertPin, world: WorldId, secret: Secret },
    /// Made by a member: the machine that joins with it is admitted on the
    /// strength of the endorsement, signed with `seed`.
    Invitation {
        hub: Vec<SocketAddr>,
        cert: CertPin,
        world: WorldId,
        founder: PublicKey,
        endorsement: Signed,
        seed: Secret,
    },
}

impl Code {
    /// The code as one word: it passes through every shell unquoted.
    pub fn to_text(&self) -> String {
        let json = serde_json::to_vec(self).expect("codes always serialize");
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let body = text.strip_prefix(PREFIX).ok_or_else(|| format!("a World code starts with {PREFIX}"))?;
        let json = URL_SAFE_NO_PAD.decode(body.as_bytes()).map_err(|error| format!("the code is damaged: {error}"))?;
        serde_json::from_slice(&json).map_err(|error| format!("the code is damaged: {error}"))
    }

    pub fn hub(&self) -> &[SocketAddr] {
        match self {
            Code::Founding { hub, .. } | Code::Invitation { hub, .. } => hub,
        }
    }

    pub fn cert(&self) -> CertPin {
        match self {
            Code::Founding { cert, .. } | Code::Invitation { cert, .. } => *cert,
        }
    }

    pub fn world(&self) -> &WorldId {
        match self {
            Code::Founding { world, .. } | Code::Invitation { world, .. } => world,
        }
    }
}
