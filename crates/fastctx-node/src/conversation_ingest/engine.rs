//! Issued source steps connect decoder progress, canonical reduction and commit.
//! No host bytes or local consumer indexes advance outside the writer transaction.
use super::{
    bodies::Bodies,
    checkpoints,
    claim::Claim,
    facts::PreparedFact,
    indexes::Indexes,
    plan::{Destination, Step},
    publisher, reading,
    source_io::StepObserver,
    step_content::StepContent,
};
use fastctx_hosts::claude::ingest::{
    body::{BodyKey, BodyPart},
    checkpoint::{Anchor, Partial},
    jsonl::Jsonl,
    reconcile::{self, Change},
};
use fastctx_sync::{
    conversation::producer::{self, InitialState, MergeBudget, MergeInput, MergeRequest, SourceSeed},
    cursor::{ByteCount, Count, Sequence},
    metrics::{Cause, observe::Observer},
    registry::REGISTRY,
    store::{
        Error,
        ingest::{IngestOperation, SourceIngest},
        ingest_effects::{DirtyProjection, SourceEffects, SourceFamily},
        source_step::{CancelProgress, SourceStep},
    },
};
use std::path::Path;

pub enum Outcome {
    More,
    Idle,
}
pub(crate) async fn advance(
    node: &crate::Node,
    claim: &Claim,
    path: &Path,
    observer: Option<&'static Observer>,
) -> Result<Outcome, Error> {
    let foundation = node.foundation.as_ref().ok_or_else(|| Error::Unavailable("node foundation is closed".into()))?;
    let source = foundation.sources();
    let expected = source.checkpoint(claim.primary.clone()).await?;
    let generation = expected.as_ref().map_or_else(|| claim.initial_generation(), |cp| cp.generation.clone());
    let step = source.begin_step(claim.primary.clone(), expected.as_ref().map(|cp| cp.revision), generation).await?;
    let result = match run(source, claim, path, observer, &step, expected).await {
        Ok(Some((operation, staged, usage, _window))) => {
            let next = operation.next().clone();
            match source.ingest(operation).await {
                Ok(receipt) => {
                    if let Some(observer) = observer {
                        observer.committed(&next, staged, &receipt, usage.as_ref());
                    }
                    foundation.wake();
                    Ok(Outcome::More)
                }
                Err(error) => Err(error),
            }
        }
        Ok(None) => {
            cancel(source, step.clone()).await?;
            Ok(Outcome::Idle)
        }
        Err(error) => Err(error),
    };
    if result.as_ref().is_err_and(|error| !matches!(error, Error::Busy)) {
        cancel(source, step).await?;
    }
    result
}

type Planned = Option<(IngestOperation, ByteCount, Option<producer::MergeUsage>, super::step_content::SuppliedContent)>;
async fn run(
    source: &dyn SourceIngest,
    claim: &Claim,
    path: &Path,
    observer: Option<&'static Observer>,
    step: &SourceStep,
    expected: Option<fastctx_sync::store::ingest::SourceCheckpoint>,
) -> Result<Planned, Error> {
    let native = source.bind_target(step.clone(), claim.primary.clone()).await?;
    let content = StepContent::open(source, native.clone(), expected.as_ref().map(|cp| cp.revision)).await?;
    let previous = match &expected {
        Some(cp) => Some(checkpoints::load(&content, cp).await?),
        None => None,
    };
    if let Some(old) = previous.as_ref()
        && matches!(old.checkpoint().partial, Some(Partial::Publishing { .. }))
    {
        let canonical = source.bind_target(step.clone(), claim.canonical.clone()).await?;
        let mut next = old.checkpoint().clone();
        let unit = publisher::unit(source, claim, &content, &next).await?;
        let mut committed = expected.as_ref().ok_or(Error::Busy)?.committed_offset;
        if unit.finished {
            let Some(Partial::Publishing { row, .. }) = next.partial.take() else { return Err(Error::Busy) };
            committed = row.end;
            let mut decoder = Jsonl::new(
                BodyKey {
                    source: claim.primary.source.clone(),
                    generation: step.generation().clone(),
                    line_start: row.end,
                    part: BodyPart::Raw,
                },
                ByteCount(row.line.0 + 1),
            );
            next.partial = Some(Partial::Reading {
                line: decoder.checkpoint(&mut Bodies { content: &content }).await.map_err(reading::error)?,
            });
        }
        let mut facts = None;
        let mut effects = SourceEffects::new(unit.effects, Vec::new(), Vec::new())?;
        let mut usage = None;
        let supplied = content.snapshot();
        if let Some(fact) = unit.fact {
            let after = source.progress(claim.canonical.clone()).await?.map_or(Sequence(0), |p| p.cursor().committed);
            let input = MergeInput {
                event: PreparedFact::input(
                    &canonical,
                    Count(0),
                    Sequence(after.0.checked_add(1).ok_or(Error::Busy)?),
                    unit.accepted.ok_or(Error::Busy)?,
                    fact,
                )?
                .event()
                .clone(),
                causes: Vec::new(),
                agent_ancestors: Vec::new(),
            };
            let initial = InitialState {
                active_branch: claim.conversation.clone(),
                sources: vec![SourceSeed {
                    source: claim.primary.source.clone(),
                    generation: claim.initial_generation(),
                }],
            };
            let dirty = DirtyProjection {
                kind: REGISTRY.projection("derived.timeline").map_err(Error::Invalid)?,
                object: claim.canonical.custody.object.clone(),
            };
            effects = SourceEffects::new(effects.indexes().to_vec(), vec![dirty.clone()], Vec::new())?;
            let budget = MergeBudget { max_work_bytes: supplied.work_bytes(1024 * 1024), ..Default::default() };
            let mut prepared = producer::plan_step(
                source,
                MergeRequest {
                    target: &canonical,
                    conversation: &claim.conversation,
                    after,
                    inputs: std::slice::from_ref(&input),
                    supplied: &supplied,
                    initial: Some(&initial),
                    other_effects: effects,
                    budget: budget.clone(),
                },
            )
            .await
            .map_err(merge_error)?;
            if prepared.progress().consumed_inputs != 1 {
                // Repair work can exhaust the unit budget before consuming input.
                // Replan without that input's local effects and leave its cursor.
                next = old.checkpoint().clone();
                committed = expected.as_ref().ok_or(Error::Busy)?.committed_offset;
                prepared = producer::plan_step(
                    source,
                    MergeRequest {
                        target: &canonical,
                        conversation: &claim.conversation,
                        after,
                        inputs: &[],
                        supplied: &supplied,
                        initial: Some(&initial),
                        other_effects: SourceEffects::new(Vec::new(), vec![dirty], Vec::new())?,
                        budget,
                    },
                )
                .await
                .map_err(merge_error)?;
                if prepared.usage().records == 0 && prepared.usage().effects == 0 {
                    return Err(Error::Busy);
                }
            }
            let output = prepared.publish(source).await.map_err(merge_error)?;
            facts = output.facts;
            effects = output.effects;
            usage = Some(output.usage);
        }
        let state = checkpoints::save_after_merge(source, &native, &next, &supplied).await?;
        let staged = next.staged_offset().map_err(reading::error)?;
        let operation = Step {
            step: step.clone(),
            expected,
            committed_offset: committed,
            state,
            previous_state: previous,
            destinations: vec![Destination { target: canonical, facts, effects }],
        }
        .prepare()?;
        return Ok(Some((operation, staged, usage, supplied)));
    }
    let observation = StepObserver { observer, step };
    let mut indexes = Indexes::new(source, &claim.primary);
    let window = if previous.as_ref().is_some_and(|cp| cp.checkpoint().read_ahead.is_some()) {
        None
    } else {
        let mut file =
            match observation.open(path, if expected.is_none() { Cause::InitialImport } else { Cause::Ordinary }) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(reading::error(error)),
            };
        let stamp = file.file.snapshot().and_then(|s| s.change_stamp()).map_err(reading::error)?;
        if let Some(old) = previous.as_ref() {
            let cp = old.checkpoint();
            let mut change = reconcile::classify(cp, Some((file.file.identity(), &stamp)), None, false);
            if change == Change::CheckAnchors {
                let mut matched = true;
                for name in ["head", "tail"] {
                    if let Some(anchor) = indexes.get::<Anchor>(SourceFamily::HostRows, &anchor_key(step, name)).await?
                    {
                        matched &= file.check_anchor(&anchor, Cause::Integrity).map_err(reading::error)?;
                    }
                }
                change = reconcile::classify(cp, Some((file.file.identity(), &stamp)), Some(matched), false);
            }
            match change {
                Change::Unchanged => return Ok(None),
                Change::Reconcile(_) => {
                    return Err(Error::Unavailable("native source changed and requires prefix reconciliation".into()));
                }
                Change::Append { .. } => {}
                _ => return Err(Error::Invalid("native source is not ready for following".into())),
            }
        }
        let offset = previous.as_ref().map_or(ByteCount(0), |cp| cp.checkpoint().read_offset);
        if offset >= stamp.size {
            return Ok(None);
        }
        let window = file
            .read_window(offset, 64 * 1024, if expected.is_none() { Cause::InitialImport } else { Cause::Ordinary })
            .map_err(reading::error)?;
        let size = window.bytes.len();
        if size == 0 {
            return Ok(None);
        }
        let tail = size.min(4096);
        let anchor = Anchor {
            offset: ByteCount(offset.0 + (size - tail) as u64),
            bytes: tail as u32,
            digest: fastctx_sync::identity::Digest::of(&window.bytes[size - tail..]),
            row_boundary: offset,
        };
        indexes.put(SourceFamily::HostRows, &anchor_key(step, "tail"), &anchor).await?;
        if offset.0 == 0 {
            let head = Anchor {
                offset,
                bytes: tail as u32,
                digest: fastctx_sync::identity::Digest::of(&window.bytes[..tail]),
                row_boundary: offset,
            };
            indexes.put(SourceFamily::HostRows, &anchor_key(step, "head"), &head).await?;
        }
        Some(window)
    };
    let advanced = reading::advance(&content, previous.as_ref().map(|cp| cp.checkpoint()), window).await?;
    let state = checkpoints::save(&content, &advanced.checkpoint).await?;
    let staged = advanced.checkpoint.staged_offset().map_err(reading::error)?;
    let operation = Step {
        step: step.clone(),
        expected,
        committed_offset: advanced.committed,
        state,
        previous_state: previous,
        destinations: vec![Destination {
            target: native,
            facts: None,
            effects: SourceEffects::new(indexes.effects(), Vec::new(), Vec::new())?,
        }],
    }
    .prepare()?;
    Ok(Some((operation, staged, None, content.snapshot())))
}
fn anchor_key(step: &SourceStep, name: &str) -> String {
    format!("claude/anchor/{}/{name}", step.generation().as_str())
}
fn merge_error(error: producer::MergeError) -> Error {
    match error {
        producer::MergeError::Store(e) => e,
        other => Error::Invalid(other.to_string()),
    }
}
async fn cancel(source: &dyn SourceIngest, step: SourceStep) -> Result<(), Error> {
    while matches!(source.cancel_step(step.clone(), 128).await?, CancelProgress::Draining { .. }) {
        tokio::task::yield_now().await;
    }
    Ok(())
}
