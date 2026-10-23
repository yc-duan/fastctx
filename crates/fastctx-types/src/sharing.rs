//! Temporary English copy for whole-machine management; shared by all entry points.

pub const HOLDER_REQUIRED: &str = "This account does not hold the machine.";
pub const VISITOR_GIVES_NOTHING: &str = "Visitors cannot share machines or manage assistance.";
pub const HUB_NOT_SHARED: &str = "The hub machine cannot be shared.";
pub const RECALL_BEFORE_MOVE: &str = "Revoke this machine's shares before moving the hub.";
pub const MANAGER_REQUIRED: &str = "This account cannot manage the machine.";

/// A committed machine change for one of its holders. The id is the existing
/// hub machine-log sequence; consumers deduplicate by (hub, account, id).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Notice {
    pub id: u64,
    pub at: jiff::Timestamp,
    pub account: crate::AccountName,
    pub machine: crate::MachineName,
    /// None means the hub revoked a legacy share of its own machine.
    pub by: Option<crate::AccountName>,
    pub action: Change,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    Rename {
        to: crate::MachineName,
    },
    Updated {
        from: String,
        to: String,
    },
    Remove {
        then: Option<crate::uninstall::Then>,
    },
    Share {
        to: crate::AccountName,
        shared: bool,
    },
    Assist {
        to: crate::AccountName,
        agent: crate::AgentName,
        home: Option<std::path::PathBuf>,
        assisting: bool,
        until: Option<jiff::Timestamp>,
    },
}
