//! The World's observations used by autonomous goals and their reviewers.

mod activity;
pub(crate) mod compact;
pub(crate) mod debts;
mod diagnostics;
mod identity;
pub(crate) mod quota;
mod records;
pub(crate) mod review;
mod stopping;
pub(crate) mod waiting;
pub(crate) mod words;
pub(crate) use activity::Activities;
pub(crate) use stopping::{classify, remember, source};

use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::session::{SessionEvent, TurnEnd};

pub(crate) fn observe(node: &Node, resident: &ResidentId, event: &SessionEvent) -> std::io::Result<()> {
    node.turn_activity.observe(resident, event);
    if matches!(
        event.happened,
        fastctx_types::session::Happened::TurnStarted { .. } | fastctx_types::session::Happened::Heard { .. }
    ) {
        crate::goals::engine::input_changed(node, resident);
    }
    diagnostics::observe(node, resident, event)
}

pub(crate) fn freeze(node: &Node, resident: &ResidentId, turn: &str, end: &mut TurnEnd) {
    if let Some(activity) = node.turn_activity.get(resident).filter(|activity| activity.turn == turn) {
        let facts = end.facts.get_or_insert_with(Default::default);
        facts.world_started_at = Some(activity.began);
        facts.world_last_activity_at = Some(activity.last);
        facts.world_progress = activity.meaningful;
        facts.world_compacted = activity.compacted;
    }
}

pub(crate) fn ended(node: &Node, resident: &ResidentId, turn: &str) {
    node.turn_activity.forget(resident, turn);
}

pub(crate) async fn snapshot(
    node: &std::sync::Arc<Node>,
    resident: &ResidentId,
    requested: Option<String>,
    refresh_limits: bool,
) -> Result<fastctx_types::turn_evidence::TurnEvidence, fastctx_types::ipc::Failure> {
    let _place = node.places.using(resident.as_str()).await;
    snapshot_locked(node, resident, requested, refresh_limits).await
}

/// The caller holds the conversation's place gate.
pub(crate) async fn snapshot_locked(
    node: &std::sync::Arc<Node>,
    resident: &ResidentId,
    requested: Option<String>,
    refresh_limits: bool,
) -> Result<fastctx_types::turn_evidence::TurnEvidence, fastctx_types::ipc::Failure> {
    use fastctx_types::ipc::Failure;
    use fastctx_types::session::Happened;
    use fastctx_types::turn_evidence::TurnEvidence;
    node.residents.active(resident)?;
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown conversation"))?;
    let now = node.residents.now(resident);
    let mut turn = requested.or_else(|| now.as_ref().and_then(|now| now.turn.clone().or(now.last_turn.clone())));
    let directory = node.world.conversation(resident);
    if turn.is_none() {
        turn = crate::delivery::terminals(&directory)
            .map_err(|error| Failure::new(error.to_string()))?
            .into_iter()
            .max_by_key(|finished| finished.queue_order)
            .map(|finished| finished.turn);
    }
    let actions = records::actions(node, resident).map_err(|error| Failure::new(error.to_string()))?;
    let mut waiting = waiting::collect(node, resident, &actions).await?;
    waiting::measure_growth(node, resident, &mut waiting).map_err(|error| Failure::new(error.to_string()))?;
    let read = || -> std::io::Result<_> {
        let finished = turn.as_deref().map(|turn| crate::delivery::terminal(&directory, turn)).transpose()?.flatten();
        let mut end = finished.as_ref().map(|finished| finished.end.clone());
        if let (Some(turn), Some(end)) = (turn.as_deref(), end.as_mut())
            && let Some(error) = diagnostics::get(node, resident, turn)?
        {
            diagnostics::augment(end, error);
        }
        let requested = turn.as_deref().map(|turn| source(node, resident, turn)).transpose()?.flatten();
        let stop = end.as_ref().map(|end| classify(end, requested, about.host.agent.as_str()));
        let activity =
            node.turn_activity.get(resident).filter(|activity| Some(activity.turn.as_str()) == turn.as_deref());
        let facts = end.as_ref().and_then(|end| end.facts.as_deref());
        let started_at =
            activity.as_ref().map(|activity| activity.began).or_else(|| facts.and_then(|facts| facts.world_started_at));
        let last_activity_at = activity
            .as_ref()
            .map(|activity| activity.last)
            .or_else(|| facts.and_then(|facts| facts.world_last_activity_at));
        let calls =
            turn.as_deref().map(|turn| crate::run_facts::evidence(&directory, turn)).transpose()?.unwrap_or_default();
        let jobs = records::jobs(node, resident)?
            .into_iter()
            .filter(|job| job.record.turn.is_some() && job.record.turn == turn)
            .collect();
        let notices = crate::delivery::history(&directory)?.into_iter().filter(|event| {
            matches!(&event.happened, Happened::Heard { turn: Some(at), from: Some(fastctx_types::session::Speaker::World), .. } if Some(at.as_str()) == turn.as_deref())
        }).collect();
        Ok(TurnEvidence {
            resident: resident.clone(),
            turn: turn.clone(),
            started_at,
            ended_at: finished.as_ref().map(|finished| finished.ended_at.unwrap_or(finished.at)),
            end_recorded_at: finished.as_ref().map(|finished| finished.at),
            last_activity_at,
            end,
            stop,
            actions: actions.into_iter().filter(|action| action.turn.is_some() && action.turn == turn).collect(),
            calls,
            jobs,
            job_updates: crate::run_facts::job_updates(&directory)?
                .into_iter()
                .filter(|update| update.turn.is_some() && update.turn == turn)
                .collect(),
            notices,
            waiting,
            quota: None,
        })
    };
    let mut evidence = read().map_err(|error| Failure::new(error.to_string()))?;
    if refresh_limits && now.as_ref().is_some_and(|now| now.turn.is_none()) {
        evidence.quota = Some(quota::read(node, resident).await);
    }
    Ok(evidence)
}
