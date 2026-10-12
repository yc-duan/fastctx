//! Telling whoever asked for an apply or unapply each step as it goes: the
//! program put in place, each of Claude Code's commands, the files written
//! together, the host's own tools checked off.

use std::sync::Arc;

pub use fastctx_types::logins::{ChangeStep, Step, StepState};

use super::{Error, Report, ToolCheck};

/// Told each step's state as it changes; a step told again replaces its
/// earlier state.
pub type Progress = Arc<dyn Fn(ChangeStep) + Send + Sync>;

pub(super) fn tell(progress: &Option<Progress>, step: Step, state: StepState) {
    if let Some(progress) = progress {
        progress(ChangeStep { step, state });
    }
}

/// A change handed to the installed program, whose own steps are not seen
/// here: told as one writing of the files.
pub(super) fn delegated(
    progress: &Option<Progress>,
    delegate: impl FnOnce() -> Result<Option<Report>, Error>,
) -> Result<Option<Report>, Error> {
    let delegated = delegate();
    if let Ok(Some(report)) = &delegated {
        tell(progress, Step::Files { changed: report.changed.clone() }, StepState::Done);
    }
    delegated
}

/// A check of the host's own tools, told once it is made.
pub(super) fn checked(progress: &Option<Progress>, check: impl FnOnce() -> ToolCheck) -> ToolCheck {
    let check = check();
    let state = match &check.result {
        Ok(()) => StepState::Done,
        Err(reason) => StepState::Failed { reason: reason.clone() },
    };
    tell(progress, Step::Check { scope: check.scope.clone() }, state);
    check
}
