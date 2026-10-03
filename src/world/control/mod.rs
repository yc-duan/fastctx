//! Typed, local-only operations for the World control terminal.
//!
//! The terminal invokes a short-lived helper over stdin/stdout, keeping slow I/O and the
//! initial enrollment's key material out of the UI process. Enrolled operations delegate to
//! the node's private admin endpoint; snapshots never trigger a hub request.

mod actions;
mod snapshot;

use super::client::NodeView;
use super::grant::{Grant, GrantSnapshot};
use super::hub::HubStatus;
use super::messages::EventsResult;
use super::node::admin::NodeStatus;
use super::{NetworkMode, WorldConfig};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::PathBuf;

/// One failure classification for UI presentation; details retain the underlying diagnosis.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureKind {
    Unavailable,
    Permission,
    Invalid,
    Timeout,
    Other,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Failure {
    pub(crate) kind: FailureKind,
    pub(crate) detail: String,
    /// A write may have reached its destination; do not automatically replay it.
    pub(crate) uncertain: bool,
}

impl Failure {
    pub(crate) fn new(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let lower = detail.to_lowercase();
        let kind = if [
            "permission denied",
            "access is denied",
            "access denied",
            "os error 5)",
            "os error 13)",
            "拒绝访问",
            "拒絕存取",
            "forbidden",
        ]
        .iter()
        .any(|word| lower.contains(word))
        {
            FailureKind::Permission
        } else if lower.contains("within")
            || lower.contains("timed out")
            || lower.contains("timeout")
        {
            FailureKind::Timeout
        } else if lower.contains("node_service_not_running")
            || lower.contains("not enrolled")
            || lower.contains("unreachable")
        {
            FailureKind::Unavailable
        } else {
            FailureKind::Other
        };
        Self {
            kind,
            detail,
            uncertain: false,
        }
    }

    pub(crate) fn invalid(detail: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Invalid,
            detail: detail.into(),
            uncertain: false,
        }
    }
}

impl From<String> for Failure {
    fn from(detail: String) -> Self {
        Self::new(detail)
    }
}

/// Missing data is distinct from unreadable data, including permission failures.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub(crate) enum ReadState<T> {
    Missing,
    Ready(T),
    Failed(Failure),
}

impl<T> ReadState<T> {
    pub(crate) fn optional(result: Result<Option<T>, String>) -> Self {
        match result {
            Ok(Some(value)) => Self::Ready(value),
            Ok(None) => Self::Missing,
            Err(error) => Self::Failed(error.into()),
        }
    }
    pub(crate) fn result(result: Result<T, String>) -> Self {
        match result {
            Ok(value) => Self::Ready(value),
            Err(error) => Self::Failed(error.into()),
        }
    }
    pub(crate) fn get(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// A node-owned, authenticated local view. Private signing material is never serialized here.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeSnapshot {
    pub(crate) status: NodeStatus,
    pub(crate) nodes: ReadState<Vec<NodeView>>,
    pub(crate) grants: GrantSnapshot,
    pub(crate) description: ReadState<String>,
    pub(crate) unverified: Vec<(String, String)>,
}

/// One complete local observation. Individual failures leave the other sections usable.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Snapshot {
    pub(crate) observed_at: String,
    pub(crate) config: ReadState<WorldConfig>,
    pub(crate) node: ReadState<NodeSnapshot>,
    pub(crate) last_status: ReadState<NodeStatus>,
    pub(crate) description: ReadState<String>,
    pub(crate) service_installed: ReadState<bool>,
    pub(crate) hub: ReadState<HubStatus>,
    pub(crate) hub_data: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServiceAction {
    Install,
    Start,
    Restart,
    Stop,
    Uninstall,
}

/// Every helper request is a single explicit user operation or a local snapshot read.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Action {
    Snapshot {
        hub_data: PathBuf,
    },
    Refresh,
    Reconnect,
    Events {
        since: u64,
        limit: u32,
    },
    Init {
        hub: String,
        bootstrap: String,
        name: String,
        tags: Vec<String>,
        network: NetworkMode,
        interface: Option<String>,
        install_service: bool,
    },
    Enroll {
        invite: String,
        name: String,
        tags: Vec<String>,
        network: NetworkMode,
        interface: Option<String>,
        install_service: bool,
    },
    Description {
        text: String,
    },
    PrivateNote {
        name: String,
        text: String,
    },
    Invite {
        name: Option<String>,
        hours: u32,
        hubs: Vec<String>,
    },
    Revoke {
        name: String,
    },
    Grant {
        grant: Grant,
        expected_revision: u64,
    },
    DeleteGrant {
        id: String,
        expected_revision: u64,
    },
    Service {
        action: ServiceAction,
    },
    Leave,
    HubRevoke {
        data: PathBuf,
        name: String,
    },
    /// Opens a project: the first version of a town's charter, written by the user.
    OpenProject {
        town: String,
        change: crate::town::ledger::CharterChange,
    },
    ExportInvite {
        path: PathBuf,
        text: String,
    },
}

impl Action {
    pub(crate) fn is_read(&self) -> bool {
        matches!(self, Self::Snapshot { .. } | Self::Events { .. })
    }
    pub(crate) fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(match self {
            Self::Snapshot { .. } => 8,
            Self::Init { .. } | Self::Enroll { .. } | Self::Leave | Self::Service { .. } => 90,
            _ => 50,
        })
    }
}

/// A completed write, including partial completion and the facts needed to recover.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Receipt {
    pub(crate) facts: Vec<(String, String)>,
    pub(crate) warnings: Vec<String>,
    pub(crate) enrolled: bool,
    pub(crate) left: bool,
    pub(crate) invite: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub(crate) enum Reply {
    Snapshot(Box<Snapshot>),
    Events(EventsResult),
    Receipt(Receipt),
    Failed(Failure),
}

/// Internal entry point. Secret inputs travel through stdin, never through argv or logs.
pub(crate) async fn run_helper() -> Result<std::process::ExitCode, String> {
    const MAX_REQUEST: u64 = 64 * 1024;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read the local control request: {error}"))?;
    let reply = if bytes.len() as u64 > MAX_REQUEST {
        Reply::Failed(Failure::invalid("The local control request is too large."))
    } else {
        match serde_json::from_slice::<Action>(&bytes) {
            Ok(action) => match crate::control::paths::ControlPaths::discover() {
                Ok(paths) => match actions::execute(&paths, action).await {
                    Ok(reply) => reply,
                    Err(failure) => Reply::Failed(failure),
                },
                Err(error) => Reply::Failed(error.into()),
            },
            Err(_) => Reply::Failed(Failure::invalid(
                "The local control request is not valid JSON.",
            )),
        }
    };
    use zeroize::Zeroize;
    bytes.zeroize();
    let result = serde_json::to_vec(&reply)
        .map_err(|error| format!("Cannot encode the local control result: {error}"))?;
    std::io::stdout()
        .write_all(&result)
        .map_err(|error| format!("Cannot write the local control result: {error}"))?;
    Ok(std::process::ExitCode::SUCCESS)
}
