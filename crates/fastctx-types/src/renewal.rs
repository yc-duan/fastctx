//! Local maintenance, independent of the World IPC and network protocols.
//!
//! The first frame is a `Request`, with `maintenance: 1`. These names and
//! meanings are frozen: future incompatible business protocols still serve
//! this envelope. Only the local user's authenticated IPC endpoint accepts it.

use serde::{Deserialize, Serialize};

use crate::ProcessIdentity;

pub const MAINTENANCE: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    pub version: String,
    pub protocol: u32,
    /// SHA-256 of the executable, distinguishing development builds that
    /// have the same package version and protocol.
    pub digest: String,
    /// When its source was made, in Unix seconds: what orders two builds of
    /// the same package version. Builds made before this was recorded, and
    /// builds from a source with no time, have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mismatch {
    pub node_version: String,
    pub node_protocol: u32,
    pub client_version: String,
    pub client_protocol: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub maintenance: u32,
    pub action: Action,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Status,
    /// Drains turns and calls. The installer first prepares a successor.
    Yield,
    Cancel,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Response {
    Status { node: Running },
    Done,
    Refused { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Running {
    pub build: Build,
    /// The stable World program this node runs from.
    pub executable: String,
    pub process: ProcessIdentity,
    pub yielding: bool,
    /// A user-owned successor keeps serving while the protected guardian
    /// is replaced. Its process is outside the old guardian's job.
    pub bridge: bool,
    pub supervised: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Staged,
    Preparing,
    WaitingForIdle,
    AwaitingLegacyExit,
    AwaitingAdministrator,
    UpdatingGuardian,
    ReturningToService,
    Complete,
    Failed,
}

/// What an interface can inspect even when its business protocol is refused.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub client: Option<Build>,
    #[serde(default)]
    pub installed: Option<Build>,
    #[serde(default)]
    pub commands: Vec<PathCommand>,
    pub node: Option<Running>,
    pub mismatch: Option<Mismatch>,
    pub pending: Option<Build>,
    pub phase: Option<Phase>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathLookup {
    Windows,
    Bash,
    Unix,
    Codex,
}

/// A terminal's first PATH entry, its resolved executable when known, and
/// the kernel identity. A wrapper that could not be resolved stays explicit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PathCommand {
    pub lookup: PathLookup,
    pub path: String,
    pub executable: Option<String>,
    pub build: Option<Build>,
    pub is_world: Option<bool>,
    pub legacy_unapply: bool,
    pub error: Option<String>,
}
