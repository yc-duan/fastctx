//! Refresh autonomous goal promises while preserving ordinary reply waits and reminder deadlines.

use super::{policy::Decision, words};
use fastctx_types::{
    goals::{
        Goal,
        runtime::{Task, WaitKey},
    },
    turn_evidence::{Liveness, WaitHandle, WaitKind},
};

fn same_target(key: &WaitKey, handle: &WaitHandle) -> bool {
    key.id == handle.id && key.machine == handle.machine && handle.kind == WaitKind::Colleague
}

fn goal_handle(handle: &WaitHandle) -> bool {
    handle.kind == WaitKind::Colleague
        && handle.completion.is_some()
        && !matches!(handle.liveness, Liveness::Unknown { .. })
}

fn key(handle: &WaitHandle) -> WaitKey {
    WaitKey {
        machine: handle.machine.clone(),
        id: handle.id.clone(),
        kind: Some(handle.kind),
        completion: handle.completion.clone(),
    }
}

/// A newer accepted activation replaces the old promise even without another requester turn.
/// Only an observed result in the requester's custody resets consecutive advice.
pub(super) fn refresh(current: &Goal, handles: &[WaitHandle]) -> Option<Decision> {
    if current
        .runtime
        .pending
        .as_ref()
        .is_some_and(|work| !matches!(work.task, Task::Idle { .. } | Task::Remind { .. }))
    {
        return None;
    }
    let mut waiting = Vec::new();
    let mut received = false;
    for existing in &current.runtime.waiting {
        if existing.kind != Some(WaitKind::Colleague) || existing.completion.is_none() {
            waiting.push(existing.clone());
            continue;
        }
        if let Some(handle) = handles.iter().find(|handle| same_target(existing, handle) && goal_handle(handle)) {
            if matches!(handle.liveness, Liveness::Ended) {
                received = true;
            } else {
                waiting.push(key(handle));
            }
        } else if handles
            .iter()
            .any(|handle| same_target(existing, handle) && matches!(handle.liveness, Liveness::Unknown { .. }))
        {
            waiting.push(existing.clone());
        }
    }
    for handle in handles
        .iter()
        .filter(|handle| goal_handle(handle) && handle.will_wake && !matches!(handle.liveness, Liveness::Ended))
    {
        let key = key(handle);
        if !waiting.contains(&key) {
            waiting.push(key);
        }
    }
    if waiting == current.runtime.waiting {
        return None;
    }
    let mut goal = current.clone();
    goal.runtime.waiting = waiting;
    if received {
        goal.runtime.advice_sent = 0;
    }
    if goal.runtime.pending.as_ref().is_some_and(|work| {
        matches!(work.task, Task::Idle { .. })
            || (goal.runtime.waiting.is_empty() && matches!(work.task, Task::Remind { .. }))
    }) {
        goal.runtime.pending = None;
    }
    Some(Decision { goal, kind: "waiting_changed", reason: words::WAITING_CHANGED.into() })
}
