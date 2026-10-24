//! Keep a lost mutating RPC distinct from a host refusal through the control layer.

use fastctx_types::ipc::Failure;

#[derive(Debug)]
pub struct ControlError(Failure);

impl std::fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl ControlError {
    pub fn failure(self) -> Failure {
        self.0
    }

    pub(super) fn uncertain(message: impl Into<String>) -> Self {
        Self(Failure::control_uncertain(message))
    }
}

impl From<String> for ControlError {
    fn from(message: String) -> Self {
        Self(Failure::new(message))
    }
}

impl From<&str> for ControlError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<super::server::Unanswered> for ControlError {
    fn from(error: super::server::Unanswered) -> Self {
        match error {
            super::server::Unanswered::Stopped(reason) => {
                Self::uncertain(format!("{reason}; the control's outcome is unknown and it will not be repeated"))
            }
            super::server::Unanswered::Refused(reason) => Self(Failure::not_run(reason)),
            error => Self(Failure::new(error.to_string())),
        }
    }
}
