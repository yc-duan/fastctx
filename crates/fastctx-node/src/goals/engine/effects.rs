//! Timed effects re-read accepted goal work, and preserve its identity through replay.

use super::{checkpoint, clock, commit, driver, failure, generation, new_work, policy, words};
use crate::Node;
use crate::timers::store::Store;
use fastctx_types::goals::View;
use fastctx_types::goals::runtime::{Task, Verdict, Work};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::session::Speaker;
use fastctx_types::timers::{Action, Timer};
use std::sync::Arc;

fn matches(view: &View, goal: &str, work: &Work) -> bool {
    view.pending.is_none()
        && view.goal.as_ref().is_some_and(|current| {
            current.id == goal
                && current.retired.is_none()
                && current.runtime.pending.as_ref().is_some_and(|pending| pending.id == work.id)
        })
}

pub(crate) async fn fire(node: &Arc<Node>, store: &Store, timer: &Timer) -> Result<(), Failure> {
    let Action::Goal { resident: id, goal: identity, work: work_id } = &timer.action else { unreachable!() };
    let gate = crate::control_queue::gate(node, id);
    let action = gate.lock().await;
    let place = node.places.using(id.as_str()).await;
    if node.residents.deleted(id).is_some() || node.residents.removed(id).is_some() {
        store.complete(timer).map_err(failure)?;
        return Ok(());
    }
    let Some(timer) = store.begin(timer, clock(node).now).map_err(failure)? else {
        return Ok(());
    };
    let Some(view) = driver::current(node, id).await? else {
        store.complete(&timer).map_err(failure)?;
        return Ok(());
    };
    let Some(work) =
        view.goal.as_ref().and_then(|goal| goal.runtime.pending.as_ref()).filter(|work| work.id == *work_id).cloned()
    else {
        store.complete(&timer).map_err(failure)?;
        return Ok(());
    };
    if !matches(&view, identity, &work) {
        store.complete(&timer).map_err(failure)?;
        return Ok(());
    }
    let mut goal = view.goal.clone().expect("matched goal");
    let restarted = checkpoint::restore(node, &mut goal)?;
    if matches!(work.task, Task::Tell { .. } | Task::Retry { .. })
        && crate::inbox::knows(&node.world.conversation(id), &work.id).map_err(failure)?
    {
        let decision = match crate::lifecycle::refused::get(node, id, &work.id).map_err(failure)? {
            Some(error) => Some(policy::host_refused(goal.clone(), error.message)),
            None => policy::delivered(&goal, &work.id),
        };
        if let Some(decision) = decision {
            commit(node, &view, decision).await?;
        }
        store.complete(&timer).map_err(failure)?;
        return Ok(());
    }
    let quiet = driver::quiet(node, id).await?;
    if matches!(work.task, Task::Retry { .. }) {
        if !quiet {
            return Err(Failure::new("goal retry waits for queued conversation input"));
        }
        let exhausted = goal.runtime.retry.as_ref().is_some_and(|retry| retry.online_ms >= policy::FAILURE_LIMIT_MS);
        if restarted || exhausted {
            let stopped = goal.runtime.retry.as_ref().expect("retry checkpoint").last.clone();
            commit(node, &view, policy::failed(goal, stopped, &clock(node), new_work())).await?;
            store.complete(&timer).map_err(failure)?;
            return Ok(());
        }
    }
    match &work.task {
        Task::Tell { text, advice } => {
            if *advice {
                let facts = crate::turn_evidence::snapshot_locked(node, id, work.turn.clone(), false).await?;
                driver::waiting_known(&facts.waiting)?;
                if !quiet || !policy::waiting(&facts.waiting).is_empty() {
                    goal.runtime.pending = None;
                    commit(
                        node,
                        &view,
                        policy::Decision {
                            goal,
                            kind: "advice_cancelled",
                            reason: "Other work resumed before the suggestion was delivered.".into(),
                        },
                    )
                    .await?;
                    store.complete(&timer).map_err(failure)?;
                    return Ok(());
                }
            } else if !quiet {
                return Err(Failure::new("goal delivery waits for queued conversation input"));
            }
            tell(node, &view, &work, &timer, text).await?;
        }
        Task::Retry { stopped } => {
            tell(node, &view, &work, &timer, &words::stopped(stopped)).await?;
        }
        Task::Idle { .. } => {
            let facts = crate::turn_evidence::snapshot_locked(node, id, work.turn.clone(), false).await?;
            driver::waiting_known(&facts.waiting)?;
            if let Some(decision) = policy::idle_ready(
                &goal,
                &work.id,
                &generation(node, id),
                quiet && policy::waiting(&facts.waiting).is_empty(),
                &clock(node),
                new_work(),
            ) {
                commit(node, &view, decision).await?;
            }
        }
        Task::Remind { .. } => {
            let facts = crate::turn_evidence::snapshot_locked(node, id, work.turn.clone(), false).await?;
            driver::waiting_known(&facts.waiting)?;
            if let Some(decision) =
                policy::reminded(&goal, &work.id, &facts.waiting, quiet, generation(node, id), &clock(node), new_work())
            {
                commit(node, &view, decision).await?;
            }
        }
        Task::Quota { .. } => {
            if !quiet {
                return Err(Failure::new("quota recovery waits for the conversation to be idle"));
            }
            let reading = crate::turn_evidence::quota::read(node, id).await;
            let codex = node.residents.about(id).is_some_and(|(about, _)| about.host.agent.as_str() == "codex");
            if let Some(decision) = policy::quota_ready(
                &goal,
                &work.id,
                driver::assistance(node, &goal)?,
                Some(&reading),
                codex,
                &clock(node),
                new_work(),
            ) {
                commit(node, &view, decision).await?;
            }
        }
        Task::Review { cause, .. } => {
            let cause = *cause;
            drop(place);
            drop(action);
            let (observed, answer) = super::judging::run(node, &goal, &work, cause).await;
            let _action = gate.lock().await;
            let _place = node.places.using(id.as_str()).await;
            let Some(current) = driver::current(node, id).await? else {
                store.complete(&timer).map_err(failure)?;
                return Ok(());
            };
            if matches(&current, identity, &work) {
                let goal = current.goal.as_ref().expect("matched goal");
                let facts = crate::turn_evidence::snapshot_locked(node, id, work.turn.clone(), false).await?;
                driver::waiting_known(&facts.waiting)?;
                let quiet = driver::quiet(node, id).await? && policy::waiting(&facts.waiting).is_empty();
                use super::super::reviewer::Answer;

                let decision = match answer {
                    Answer::Judged(verdict) => {
                        policy::reviewed(goal, &work.id, verdict, observed, quiet, &clock(node), new_work())
                    }
                    Answer::Absent { reason } => policy::unreviewed(goal, &work.id, reason, quiet),
                    Answer::Offline { reason } if quiet => {
                        policy::review_unavailable(goal, &work.id, true, reason, &clock(node), new_work())
                    }
                    Answer::Failed { reason } if quiet => {
                        policy::review_unavailable(goal, &work.id, false, reason, &clock(node), new_work())
                    }
                    Answer::Offline { reason } | Answer::Failed { reason } => policy::reviewed(
                        goal,
                        &work.id,
                        Verdict::Failed { reason },
                        observed,
                        false,
                        &clock(node),
                        new_work(),
                    ),
                };
                if let Some(decision) = decision {
                    commit(node, &current, decision).await?;
                }
            }
        }
        Task::Compact => {
            if !quiet && !super::compaction::started(node, id, &work)? {
                return Err(Failure::new("goal compaction waits for queued input"));
            }
            drop(place);
            drop(action);
            let mut outcome = super::compaction::run(node, id, &work).await?;
            let _action = gate.lock().await;
            let _place = node.places.using(id.as_str()).await;
            let Some(current) = driver::current(node, id).await? else {
                store.complete(&timer).map_err(failure)?;
                return Ok(());
            };
            if let fastctx_types::turn_evidence::Compacted::Failed { reason }
            | fastctx_types::turn_evidence::Compacted::Refused { reason } = &mut outcome
                && let Some(stopped) =
                    crate::turn_evidence::snapshot_locked(node, id, work.turn.clone(), false).await?.stop
            {
                *reason = format!("{reason}\n{}", words::stopped(&stopped));
            }
            if matches(&current, identity, &work)
                && let Some(decision) = policy::compacted(
                    current.goal.as_ref().expect("matched goal"),
                    &work.id,
                    outcome,
                    &clock(node),
                    new_work(),
                )
            {
                commit(node, &current, decision).await?;
            }
        }
    }
    store.complete(&timer).map_err(failure)?;
    Ok(())
}

async fn tell(node: &Arc<Node>, view: &View, work: &Work, timer: &Timer, text: &str) -> Result<(), Failure> {
    let goal = view.goal.as_ref().expect("accepted work");
    let id = &goal.resident.resident;
    let management = node.residents.management(id)?;
    let _reading = management.read().await;
    let delivery = Delivery {
        id: work.id.clone(),
        at: timer.fired_at.expect("begun timer"),
        notice: Notice::Words { from: Speaker::World, text: text.into(), goal_report: None, goal_question: None },
    };
    let result = crate::speech::tell_here_locked(node, id, delivery, crate::speech::Owed::Nobody).await;
    drop(_reading);
    if let Err(error) = result {
        if crate::lifecycle::refused::reported(&error) {
            return commit(node, view, policy::host_refused(goal.clone(), error.message)).await;
        }
        return Err(error);
    }
    if let Some(decision) = policy::delivered(goal, &work.id) {
        commit(node, view, decision).await?;
    }
    Ok(())
}
