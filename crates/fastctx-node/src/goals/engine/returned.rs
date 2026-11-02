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

fn decide(
    node: &Node,
    goal: &Goal,
    current_turn: &Option<String>,
    handles: &[fastctx_types::turn_evidence::WaitHandle],
    new_turn: bool,
) -> Result<Option<policy::Decision>, Failure> {
    if goal.runtime.waiting.is_empty() {
        return Ok(None);
    }
    for waiting in &goal.runtime.waiting {
        if waiting.completion.is_some()
            && handles.iter().any(|handle| {
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
    let directory = node.world.conversation(&goal.resident.resident);
    let mut selections = std::collections::BTreeSet::new();
    for waiting in &goal.runtime.waiting {
        if let Some(completion) = &waiting.completion {
            selections.insert(("delivery", completion.to_string()));
            selections.insert(("completion", completion.to_string()));
        } else if new_turn {
            selections.insert(("answer-turn", crate::session_record::search::turn_key(current_turn.as_deref())));
        }
    }
    let mut events = std::collections::BTreeMap::new();
    let history = crate::session_record::search::open(&directory).map_err(failure)?;
    for (tag, value) in selections {
        events.extend(history.all(tag, &value).map_err(failure)?);
    }
    for (_, event) in events {
        let Happened::Heard { delivery, turn, notice, .. } = &event.happened else { continue };
        for waiting in &goal.runtime.waiting {
            let returned = if let Some(expected) = &waiting.completion {
                delivery.as_ref() == Some(expected) || super::super::waiting::returned(node, waiting, &event, handles)
            } else if new_turn && turn == current_turn {
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

pub(super) async fn decision(
    node: &std::sync::Arc<Node>,
    goal: &Goal,
    facts: &TurnEvidence,
    new_turn: bool,
) -> Result<Option<policy::Decision>, Failure> {
    let (node, goal, turn, handles) = (node.clone(), goal.clone(), facts.turn.clone(), facts.waiting.clone());
    fastctx_platform::task::spawn_blocking(move || decide(&node, &goal, &turn, &handles, new_turn))
        .await
        .map_err(failure)?
}
