//! A received result, rather than a producer disappearing, settles a background wait.

use super::{failure, policy};
use crate::Node;
use fastctx_types::layout::Here;
use fastctx_types::{
    goals::Goal,
    ipc::Failure,
    residents::Notice,
    session::Happened,
    turn_evidence::{Liveness, TurnEvidence, WaitKind},
};

pub(super) fn decision(
    node: &Node,
    goal: &Goal,
    facts: &TurnEvidence,
    new_turn: bool,
) -> Result<Option<policy::Decision>, Failure> {
    if goal.runtime.waiting.is_empty() {
        return Ok(None);
    }
    for waiting in &goal.runtime.waiting {
        if waiting.completion.is_some()
            && facts.waiting.iter().any(|handle| {
                handle.kind == WaitKind::Colleague
                    && matches!(handle.liveness, Liveness::Ended)
                    && handle.completion == waiting.completion
                    && handle.id == waiting.id
                    && (handle.machine == waiting.machine
                        || node.names().names_of(&handle.machine).contains(&waiting.machine))
            })
        {
            return Ok(policy::wait_returned(goal, waiting));
        }
    }
    for event in crate::delivery::history(&node.world.conversation(&goal.resident.resident)).map_err(failure)? {
        let Happened::Heard { delivery, turn, notice, .. } = &event.happened else { continue };
        for waiting in &goal.runtime.waiting {
            let returned = if let Some(expected) = &waiting.completion {
                delivery.as_ref() == Some(expected)
                    || super::super::waiting::returned(node, waiting, &event, &facts.waiting)
            } else if new_turn && turn == &facts.turn {
                matches!(notice.as_deref(), Some(Notice::Answer { colleague, .. })
                    if colleague.resident.as_str() == waiting.id
                    && (colleague.machine == waiting.machine || node.names().names_of(&colleague.machine).contains(&waiting.machine)))
            } else {
                false
            };
            if returned {
                return Ok(policy::wait_returned(goal, waiting));
            }
        }
    }
    Ok(None)
}
