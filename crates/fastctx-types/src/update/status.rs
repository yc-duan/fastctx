//! Structured update outcomes shared by devices and their interfaces.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::Via;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReachFailure {
    Dns,
    Connect,
    Tls,
    Timeout,
    Interrupted,
    Proxy,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckFailure {
    Unreachable {
        reason: ReachFailure,
    },
    RateLimited {
        until: Option<Timestamp>,
    },
    Http {
        status: u16,
    },
    Unreadable,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FailureReason {
    NoPackage,
    Sources {
        sources: Vec<SourceFailure>,
    },
    Signature,
    Disk,
    Installer {
        code: Option<i32>,
    },
    NotNewer,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFailure {
    pub via: Via,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub reason: FailureReason,
    pub why: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Stage {
    #[default]
    Idle,
    Fetching {
        version: String,
        /// None until a source actually delivers bytes.
        via: Option<Via>,
        received: u64,
        total: Option<u64>,
    },
    Ready {
        version: String,
    },
    WaitingIdle {
        version: String,
    },
    WaitingApp {
        version: String,
    },
    /// The application installs for its user, in a session of theirs: no
    /// one is signed in, and it installs once they are.
    WaitingSignIn {
        version: String,
    },
    Repairing {
        version: String,
        layer: String,
    },
    Installing {
        version: String,
    },
    Renewing {
        version: String,
        phase: crate::renewal::Phase,
    },
    Administrator {
        version: String,
    },
    Failed {
        version: String,
        problem: Problem,
    },
    #[serde(other)]
    Unknown,
}

impl Stage {
    /// The release whose work this stage describes, when there is one.
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Idle | Self::Unknown => None,
            Self::Fetching { version, .. }
            | Self::Ready { version }
            | Self::WaitingIdle { version }
            | Self::WaitingApp { version }
            | Self::WaitingSignIn { version }
            | Self::Repairing { version, .. }
            | Self::Installing { version }
            | Self::Renewing { version, .. }
            | Self::Administrator { version }
            | Self::Failed { version, .. } => Some(version),
        }
    }
}

impl Problem {
    pub fn new(reason: FailureReason, why: impl Into<String>) -> Self {
        Self { reason, why: why.into() }
    }
}
impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}
impl From<String> for Problem {
    fn from(why: String) -> Self {
        Self::new(FailureReason::Other, why)
    }
}
impl From<&str> for Problem {
    fn from(why: &str) -> Self {
        Self::new(FailureReason::Other, why)
    }
}
impl From<Problem> for String {
    fn from(problem: Problem) -> Self {
        problem.why
    }
}
