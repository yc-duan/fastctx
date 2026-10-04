//! What local clients and the node say to each other.
//!
//! A client connects to the node's endpoint, says who it is, and is welcomed
//! or refused. After that it sends calls, one at a time, each with an id it
//! chose, and the node answers each call once. A client with several calls
//! to make at once opens several connections. Every frame is one of these
//! types in its JSON form.

use serde::{Deserialize, Serialize};

use crate::residents::Host;
use crate::{JobId, MachineName, ResidentId};

/// Raised whenever a change would make an older client or node misread a
/// frame. A node refuses a client speaking another protocol.
pub const PROTOCOL: u32 = 1;

/// The first frame on every connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// The client binary's version, shown when versions differ.
    pub version: String,
    pub peer: Peer,
}

/// What is on the other end of a connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Peer {
    /// A command such as `fastctx w`, typed by a person or run by a
    /// resident; `resident` is set when a resident's `run` started it.
    Command { resident: Option<ResidentId> },
    /// The supervisor of one job, reporting on it.
    Supervisor { job: JobId },
    /// The MCP adapter of one agent session.
    Session { host: Host },
}

/// The node's answer to `Hello`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Welcome {
    Accepted { machine: MachineName, version: String },
    Refused { reason: String },
}

/// A frame from a client after the welcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ClientFrame {
    Call { id: u64, call: crate::Call },
}

/// A frame from the node after the welcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NodeFrame {
    /// The one answer to a call; nothing more comes for its id.
    Reply { id: u64, reply: Result<crate::Reply, Failure> },
}

/// Why a call did not do what it asked, in words for whoever reads the
/// answer: a person at a terminal or a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct Failure {
    pub message: String,
}

impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}
