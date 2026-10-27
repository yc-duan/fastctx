//! Stable reasons for refusing an account's action, independent of its human-readable explanation.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialReason {
    NoSubject,
    ViewUnavailable,
    AccountMissing,
    AccountLocked,
    MachineMissing,
    NotMachine,
    MachineNotShared,
    LoginNotAssisting,
    ManagedMachine,
    ProjectMissing,
    ProjectNotShared,
    MachineNotOwned,
    CannotAct,
    #[serde(other)]
    Unknown,
}

/// An authorization decision that can be shown and translated without parsing English.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct Denial {
    pub reason: DenialReason,
    pub message: String,
}

impl Denial {
    pub fn new(reason: DenialReason, message: impl Into<String>) -> Self {
        Self { reason, message: message.into() }
    }
}

impl From<Denial> for crate::ipc::Failure {
    fn from(denial: Denial) -> Self {
        Self {
            message: denial.message,
            code: Some(crate::ipc::FailureCode::Refused),
            not_run: true,
            denial: Some(denial.reason),
            capacity: None,
        }
    }
}
