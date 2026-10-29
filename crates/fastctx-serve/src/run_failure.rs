//! Run failures preserve the distinction between refusal and an unknown outcome.

use fastctx_client::CallError;

use crate::answer::Answer;

pub(crate) struct Failure {
    not_run: bool,
    message: String,
}

impl Failure {
    /// No Run call was submitted to the node.
    pub(crate) fn before_dispatch(message: String) -> Self {
        Self { not_run: true, message }
    }

    pub(crate) fn answer(self, machine: Option<&str>) -> Answer {
        if self.not_run {
            return Answer::error(format!("=== run (not run) ===\n{}", self.message));
        }
        let machine = machine.unwrap_or("the requested machine");
        Answer::error(format!(
            "=== run on {machine} (its answer was lost; whether it ran is not known) ===\n{}\n\
             If it ran, its end will come to you as a notice.",
            self.message
        ))
    }
}

impl From<CallError> for Failure {
    fn from(error: CallError) -> Self {
        Self { not_run: matches!(&error, CallError::Failed(failure) if failure.not_run), message: error.to_string() }
    }
}
