//! Freeze only the evidence and claims appropriate to the review's cause.

use super::{
    super::{
        operations,
        reviewer::{Evidence, Materials, OwnerWords},
    },
    failure,
};
use crate::Node;
use fastctx_types::goals::runtime::{ReviewCause, Work};
use fastctx_types::goals::{Goal, Owner};
use fastctx_types::ipc::Failure;
use fastctx_types::session::{Happened, Speaker};
use fastctx_types::skill_files::File;
use std::sync::Arc;

pub(super) fn owns(node: &Node, goal: &Goal, speaker: &Speaker) -> bool {
    matches!(speaker, Speaker::User { .. })
        || matches!((&goal.owner, speaker),
        (Owner::Conversation { address: owner }, Speaker::Resident { address }) if super::super::same(node, owner, address))
}

fn recent_owner_now(node: &Node, goal: &Goal) -> Result<Vec<(String, OwnerWords)>, Failure> {
    let mut words = Vec::new();
    for event in crate::delivery::history(&node.world.conversation(&goal.resident.resident)).map_err(failure)? {
        if let Happened::Heard { delivery, from: Some(speaker), text, .. } = &event.happened
            && owns(node, goal, speaker)
            && event.at >= goal.runtime.activated_at.unwrap_or(goal.created_at)
        {
            let key = delivery.as_ref().map(ToString::to_string).unwrap_or_else(|| {
                fastctx_hands::files::content_hash(&serde_json::to_vec(&event).expect("event serializes"))
            });
            words.push((key, OwnerWords { at: event.at, speaker: speaker.clone(), text: text.clone() }));
        }
    }
    words.sort_by_key(|(_, words)| words.at);
    Ok(words)
}

pub(super) async fn build(
    node: &Arc<Node>,
    goal: &Goal,
    work: &Work,
    cause: ReviewCause,
) -> Result<Materials, Failure> {
    let _place = node.places.using(goal.resident.resident.as_str()).await;
    node.residents.active(&goal.resident.resident)?;
    let directory = node.world.conversation(&goal.resident.resident);
    if let Some(materials) = super::material_store::cached(directory.clone(), work.id.to_string()).await? {
        if materials.account != goal.account || materials.conversation != goal.resident || materials.goal_id != goal.id
        {
            return Err(Failure::new("the cached review belongs to a different goal"));
        }
        let (about, _) = node
            .residents
            .about(&goal.resident.resident)
            .ok_or_else(|| Failure::new("the reviewed conversation is unavailable"))?;
        if materials.full_machine.is_none() && !node.may_work_here(&about.acting)? {
            return Err(Failure::not_run(
                "this legacy review points to the assisting host; create a new review on a working computer",
            ));
        }
        return super::material_size::cached(materials);
    }
    let (about, _) = node
        .residents
        .about(&goal.resident.resident)
        .ok_or_else(|| Failure::new("the reviewed conversation is unavailable"))?;
    let place = crate::placement::artifacts::lease(node, &goal.resident.resident, &about.origin, &about.acting).await?;
    let now = jiff::Timestamp::now();
    let key = format!(
        "goal-{}",
        fastctx_hands::files::content_hash(&serde_json::to_vec(&(&goal.id, &work.id, now)).map_err(failure)?)
    );
    let mut files = Vec::new();
    let judgments: Vec<_> = super::super::archive::decisions(node, goal)
        .await?
        .into_iter()
        .filter(|decision| {
            matches!(decision.kind.as_str(), "review_sound" | "review_advice" | "review_obsolete" | "review_failed")
        })
        .collect();
    let (judgments_file, previous_judgments) = tokio::task::spawn_blocking(move || {
        let file = File::text("previous-judgments.json", &serde_json::to_string(&judgments).map_err(failure)?);
        let words = judgments
            .iter()
            .map(|decision| format!("{} {}: {}", decision.at, decision.kind, decision.reason))
            .collect();
        Ok::<_, Failure>((file, words))
    })
    .await
    .map_err(failure)??;
    files.push(judgments_file);
    let mut result = Materials {
        account: goal.account.clone(),
        conversation: goal.resident.clone(),
        goal_id: goal.id.clone(),
        cause,
        suggestions_left: 2u8.saturating_sub(goal.runtime.advice_sent),
        objective: goal.objective.clone(),
        plan_claim: None,
        world_actions: None,
        stopped: None,
        last_words_claim: None,
        blocked_claim: None,
        owner_words: Vec::new(),
        previous_judgments,
        machines: Vec::new(),
        machines_total: None,
        now,
        full_path: Some(place.file(&key, "full-materials.json")?),
        full_machine: Some(place.location.machine.clone()),
        truncated: false,
    };
    let mut observations = None;
    if cause == ReviewCause::Paused {
        result.owner_words =
            recent_owner(node, goal).await?.into_iter().rev().take(4).map(|(_, words)| words).collect();
        result.owner_words.reverse();
    } else {
        let owner = match &goal.owner {
            Owner::User => &goal.resident.resident,
            Owner::Conversation { address } => &address.resident,
        };
        if let Some(body) = operations::bounded(node.goals.records.read(&goal.account, "plan", owner.as_str()))
            .await?
            .and_then(|record| record.body)
        {
            result.plan_claim = Some(
                body["markdown"]
                    .as_str()
                    .ok_or_else(|| Failure::new("the plan record has no Markdown body"))?
                    .to_string(),
            );
        }
        let link = node.link_of(&goal.account.hub).ok_or_else(|| Failure::new("the goal's hub is unavailable"))?;
        let view = crate::authorization::current_view(&link)?;
        result.machines = view
            .machines()
            .filter(|machine| {
                view.authorize(&goal.account.account, fastctx_net::authorization::Action::Work(&machine.name)).is_ok()
            })
            .map(|machine| machine.name.clone())
            .collect();
        let previous = match &goal.runtime.reviewed_work {
            Some(previous) => super::material_store::observations(directory.clone(), previous.to_string()).await?,
            None => crate::turn_evidence::review::Seen::default(),
        };
        let (seen, evidence) = {
            let node = node.clone();
            let resident = goal.resident.resident.clone();
            tokio::task::spawn_blocking(move || crate::turn_evidence::review::collect(&node, &resident, &previous))
                .await
                .map_err(failure)?
                .map_err(failure)?
        };
        let full_path = place.file(&key, "world-facts.json")?;
        let (summary, evidence_file, full_path) = tokio::task::spawn_blocking(move || {
            let summary = evidence.summary(&full_path);
            let file = File::text("world-facts.json", &serde_json::to_string(&evidence).map_err(failure)?);
            Ok::<_, Failure>((summary, file, full_path))
        })
        .await
        .map_err(failure)??;
        files.push(evidence_file);
        observations = Some(super::material_store::stage(&directory, seen).await?);
        result.world_actions = Some(Evidence {
            since: goal.runtime.reviewed_work.as_ref().and(goal.runtime.reviewed_through),
            through: now,
            snapshot: Some(work.id.clone()),
            summary,
            full_path,
        });
        if cause == ReviewCause::Blocked {
            result.blocked_claim = goal.statement.clone();
        }
        if cause == ReviewCause::Idle {
            let facts =
                crate::turn_evidence::snapshot_locked(node, &goal.resident.resident, work.turn.clone(), false).await?;
            result.stopped = facts.stop;
            result.last_words_claim =
                facts.end.and_then(|end| end.last_message).map(|text| text.chars().take(1200).collect());
        }
    }
    let (full, result) = tokio::task::spawn_blocking(move || {
        let full = File::text("full-materials.json", &serde_json::to_string(&result).map_err(failure)?);
        Ok::<_, Failure>((full, super::material_size::freeze(result)?))
    })
    .await
    .map_err(failure)??;
    files.push(full);
    place.publish(node, key, files).await?;
    super::material_store::install(directory, work.id.to_string(), result, observations).await
}

pub(super) async fn recent_owner(node: &Arc<Node>, goal: &Goal) -> Result<Vec<(String, OwnerWords)>, Failure> {
    let (node, goal) = (node.clone(), goal.clone());
    tokio::task::spawn_blocking(move || recent_owner_now(&node, &goal)).await.map_err(failure)?
}
