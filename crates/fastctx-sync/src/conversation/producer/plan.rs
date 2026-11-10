use super::budget::{effects, exceeded, too_large, usage};
use super::{
    api::*,
    content::Content,
    emit,
    rows::Rows,
    state::{Engine, Meta},
};
use crate::{conversation::SourceState, identity::OperationId, store::ingest::SourceIngest};
use fastctx_types::sync::conversation_timeline::{KIND, SCHEMA};
use std::collections::BTreeMap;

/// Plans a bounded prefix and resumable repair work. Nothing is persisted until
/// publish and the caller's single ingest commit both succeed.
pub async fn plan_step(source: &dyn SourceIngest, request: MergeRequest<'_>) -> Result<PreparedMerge> {
    let available = request.budget.clone();
    match Box::pin(plan(source, request)).await {
        Err(MergeError::Limit(kind)) => Err(too_large(MergeUsage::default(), &available, kind)),
        result => result,
    }
}

async fn plan(source: &dyn SourceIngest, request: MergeRequest<'_>) -> Result<PreparedMerge> {
    validate(&request)?;
    let budget = request.budget.clone();
    let meta: Option<Meta> = source
        .get_index(
            request.target.target().clone(),
            crate::store::ingest_effects::SourceFamily::Reducer,
            [super::keys::namespace(request.conversation), b"meta".to_vec()].concat(),
        )
        .await?
        .map(|row| {
            serde_json::from_slice(&row.value)
                .map_err(|e| MergeError::CorruptState { key: row.key, reason: e.to_string() })
        })
        .transpose()?;
    let initial_branch = if let Some(meta) = &meta {
        if meta.version != 1
            || meta.conversation != *request.conversation
            || request.initial.is_some_and(|seed| seed.active_branch != meta.initial_branch)
        {
            return Err(MergeError::InvalidInput("reducer version, conversation or initial branch differs".into()));
        }
        meta.initial_branch.clone()
    } else {
        request
            .initial
            .ok_or_else(|| MergeError::InvalidInput("first canonical merge requires explicit initial state".into()))?
            .active_branch
            .clone()
    };
    let mut engine = Engine {
        source,
        target: request.target.clone(),
        conversation: request.conversation.clone(),
        content: Content::new(source, request.target.clone(), request.supplied, &budget),
        rows: Rows::new(request.conversation, budget.max_work_bytes),
        changes: Vec::new(),
        inputs: Vec::new(),
        initial_branch: initial_branch.clone(),
    };
    if meta.is_none() {
        engine.put("meta", &Meta { version: 1, conversation: request.conversation.clone(), initial_branch }).await?;
    }
    if let Some(initial) = request.initial {
        for seed in &initial.sources {
            let seed_key = format!("source-seed/{}", seed.source.as_str());
            if let Some(existing) = engine.get::<crate::identity::HostGeneration>(&seed_key).await? {
                if existing != seed.generation {
                    return Err(MergeError::InvalidInput(
                        "source initial generation differs from its retained seed".into(),
                    ));
                }
            } else {
                engine.put(&seed_key, &seed.generation).await?;
            }
            let key = format!("source/{}", seed.source.as_str());
            if engine.get::<SourceState>(&key).await?.is_none() {
                engine
                    .put(
                        &key,
                        &SourceState { source: seed.source.clone(), current: seed.generation.clone(), last: None },
                    )
                    .await?;
            }
        }
    }
    let mut consumed = 0usize;
    let mut replayed = 0usize;
    let mut units = 0u32;
    let mut step_operations = BTreeMap::<OperationId, crate::manifest::ContentBlock>::new();
    let mut stopped = None;
    let mut initial_work = true;
    let mut last_usage = usage(&engine, request.after, &request.other_effects, units)?;
    if let Some(kind) = exceeded(&last_usage, &budget) {
        return Err(too_large(last_usage, &budget, kind));
    }
    loop {
        let has_work = engine.has("work/").await?;
        let do_work = has_work && (initial_work || consumed == request.inputs.len());
        if !has_work {
            initial_work = false;
        }
        if !do_work && consumed == request.inputs.len() {
            break;
        }
        if units >= budget.max_work_units {
            stopped = Some(BudgetKind::WorkUnits);
            break;
        }
        let before_rows = engine.rows.clone();
        let before_changes = engine.changes.len();
        let before_inputs = engine.inputs.len();
        let before_content = engine.content.checkpoint()?;
        let mut is_replay = false;
        let result = if do_work {
            Box::pin(engine.run_job()).await.map(|_| ())
        } else {
            let input = &request.inputs[consumed];
            if matches!(
                &input.event.fact,
                fastctx_types::sync::conversation_timeline::TimelineFact::ItemRevision { .. }
            ) {
                return Err(MergeError::InvalidInput("canonical revisions cannot be input to production".into()));
            }
            let bytes = fastctx_types::sync::transport_codec::bounded_json(&input.event, 65536)
                .map_err(|_| MergeError::Limit(BudgetKind::PayloadBytes));
            match bytes {
                Err(error) => Err(error),
                Ok(bytes) => {
                    let identity = engine.content.block_identity(&bytes)?;
                    let recorded = source
                        .recorded(request.target.target().clone(), input.event.evidence.operation.clone())
                        .await?;
                    let known =
                        step_operations.get(&input.event.evidence.operation).cloned().or(recorded.map(|r| r.payload));
                    if let Some(previous) = known {
                        if previous == identity {
                            is_replay = true;
                            Ok(())
                        } else {
                            Err(MergeError::Reduction {
                                evidence: Box::new(input.event.evidence.clone()),
                                error: crate::conversation::Error::ConflictingReplay,
                            })
                        }
                    } else {
                        let sequence = request
                            .after
                            .0
                            .checked_add(engine.inputs.len() as u64 + 1)
                            .ok_or(MergeError::CounterExhausted)?;
                        if input.event.evidence.record.stream != request.target.target().stream
                            || input.event.evidence.record.epoch != request.target.target().epoch
                            || input.event.evidence.record.sequence.0 != sequence
                            || input.event.evidence.accepted.0 == 0
                        {
                            return Err(MergeError::InvalidInput(
                                "input Evidence does not identify its own contiguous source record".into(),
                            ));
                        }
                        Box::pin(engine.apply(input, &input.event.evidence)).await.map(|()| {
                            engine.inputs.push(input.clone());
                        })
                    }
                }
            }
        };
        let trial_usage = usage(&engine, request.after, &request.other_effects, units.saturating_add(1));
        let limit = engine.content.limit().or(match &result {
            Err(MergeError::Limit(kind)) => Some(*kind),
            _ => None,
        });
        if let Err(error) = result
            && limit.is_none()
        {
            return Err(error);
        }
        let kind =
            limit.or_else(|| trial_usage.as_ref().ok().and_then(|u| exceeded(u, &budget))).or(match &trial_usage {
                Err(MergeError::Limit(kind)) => Some(*kind),
                _ => None,
            });
        if let Some(kind) = kind {
            let needed = trial_usage.unwrap_or_else(|_| {
                let mut u = last_usage.clone();
                u.work_units = units.saturating_add(1);
                u
            });
            engine.rows = before_rows;
            engine.changes.truncate(before_changes);
            engine.inputs.truncate(before_inputs);
            engine.content.rollback(before_content)?;
            if units == 0 {
                return Err(too_large(needed, &budget, kind));
            }
            stopped = Some(kind);
            break;
        }
        last_usage = trial_usage?;
        units += 1;
        if !do_work {
            let input = &request.inputs[consumed];
            let bytes = serde_json::to_vec(&input.event).map_err(|e| MergeError::InvalidInput(e.to_string()))?;
            step_operations.insert(input.event.evidence.operation.clone(), engine.content.block_identity(&bytes)?);
            consumed += 1;
            if is_replay {
                replayed += 1;
            }
        }
    }
    let ready_work = engine.has("work/").await?;
    let waiting_dependencies = engine.has("pending/").await? || engine.has("unresolved-placement/").await?;
    let facts =
        emit::encode_batch(&engine.content, &engine.target, request.after, &engine.inputs, &engine.changes, true)?;
    let (effects, _) = effects(&engine, &request.other_effects)?;
    let mut measured = usage(&engine, request.after, &request.other_effects, units)?;
    // Output payloads were accounted before staging; staging now charges their
    // retained publication buffers too, and must stay in the same admitted window.
    measured.work_bytes = measured.work_bytes.max(last_usage.work_bytes);
    if let Some(kind) = exceeded(&measured, &budget) {
        return Err(too_large(measured, &budget, kind));
    }
    let progress = MergeProgress {
        consumed_inputs: consumed,
        appended_inputs: engine.inputs.len(),
        replayed_inputs: replayed,
        ready_work,
        waiting_dependencies,
        stop: stopped.map_or(StopReason::Complete, StopReason::Budget),
    };
    let target = engine.target.clone();
    let publications = engine.content.into_publications()?;
    Ok(PreparedMerge { target, publications, output: MergeOutput { progress, usage: measured, facts, effects } })
}
fn validate(request: &MergeRequest<'_>) -> Result<()> {
    let kind = request.target.target().custody.kind.registration();
    let b = &request.budget;
    if request.target.target().custody.object.object
        != fastctx_types::sync::conversation_identity::ConversationObject::Timeline.id(request.conversation)
        || kind.id != KIND
        || kind.schema != SCHEMA
        || b.max_effects > 256
        || b.max_effect_bytes > 262144
        || b.max_records > 256
        || b.max_payload_bytes > crate::store::ingest::PAYLOAD_BYTES
        || b.max_commit_bytes > 262144
        || b.max_work_units == 0
        || b.max_content_reads == 0
        || b.max_work_bytes == 0
        || b.max_work_bytes > crate::store::ingest_memory::MAX_SOURCE_BYTES
    {
        return Err(MergeError::InvalidInput("merge target or budget exceeds ingestion bounds".into()));
    }
    let prefix = super::keys::namespace(request.conversation);
    if request
        .other_effects
        .indexes()
        .iter()
        .any(|r| r.family == crate::store::ingest_effects::SourceFamily::Reducer && r.key.starts_with(&prefix))
    {
        return Err(MergeError::InvalidInput("caller effects overlap the canonical reducer namespace".into()));
    }
    Ok(())
}
