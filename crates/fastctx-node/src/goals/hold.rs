//! A durable local hold stays in force until the account acknowledges it.
use fastctx_types::goals::{Author, Status, View};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HoldKind {
    Archived,
    Assistance,
    UserStop,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Hold {
    pub base: Option<u64>,
    pub kind: HoldKind,
    pub at: Timestamp,
    pub goal: String,
    pub operation: String,
    pub reason: String,
}

impl Hold {
    pub(crate) fn pending(&self) -> fastctx_types::goals::Pending {
        fastctx_types::goals::Pending {
            action: match self.kind {
                HoldKind::Archived => fastctx_types::goals::PendingAction::End,
                HoldKind::Assistance | HoldKind::UserStop => fastctx_types::goals::PendingAction::Pause,
            },
            reason: self.reason.clone(),
            at: self.at,
        }
    }

    pub(crate) fn superseded(&self, view: &View) -> bool {
        let Some(goal) = &view.goal else { return true };
        goal.id != self.goal
            || goal.operation == self.operation
            || goal.status == Status::Complete
            || goal.retired.is_some()
            || (matches!(self.kind, HoldKind::UserStop)
                && goal.status == Status::Paused
                && goal.runtime.clears_stop.as_ref() == Some(&self.operation))
            || (view.version > self.base
                && goal.decisions.last().is_some_and(|decision| {
                    decision.operation == goal.operation
                        && (matches!(decision.by, Author::User { .. })
                            || matches!((&decision.by, &goal.owner),
                                (Author::Resident { address }, fastctx_types::goals::Owner::Conversation { address: owner }) if address == owner))
                        && matches!(decision.kind.as_str(), "opened" | "resumed" | "paused")
                        && (!matches!(self.kind, HoldKind::UserStop)
                            || decision.kind == "paused"
                            || goal.runtime.clears_stop.as_ref() == Some(&self.operation))
                }))
    }
}
