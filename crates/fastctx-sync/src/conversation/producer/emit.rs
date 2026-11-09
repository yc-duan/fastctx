use super::{api::*, content::Content};
use crate::{
    catalog::{RecordRef, SourceBatch, SourceRecord},
    cursor::Sequence,
    identity::{Digest, OperationId},
    manifest::ContentRef,
    store::{ingest::IngestFacts, source_step::SourceStepTarget},
};
use fastctx_types::sync::{conversation_timeline::*, timeline::TimelineChange};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(super) struct Change {
    pub change: TimelineChange,
    pub trigger: Evidence,
    pub causes: Vec<RecordRef>,
}
pub(super) fn causes(values: &mut Vec<RecordRef>) {
    values.sort_by(|a, b| (&a.stream, &a.epoch, a.sequence).cmp(&(&b.stream, &b.epoch, b.sequence)));
    values.dedup();
}
pub(super) fn roots(fact: &TimelineFact) -> Result<Vec<ContentRef>> {
    let values = match fact {
        TimelineFact::Observed { observation } => {
            vec![observation.body.original.clone(), observation.body.display.clone()]
        }
        TimelineFact::TextAppend { append } => vec![append.chunk.clone()],
        TimelineFact::Run { run, .. } => run.summary.iter().cloned().collect(),
        TimelineFact::Image { picture: PictureLink { target: PictureTarget::Owned { content }, .. } } => {
            vec![content.clone()]
        }
        TimelineFact::Branch { change: BranchChange::Select { root, .. } }
        | TimelineFact::ItemRevision { change: TimelineChange::SelectBranch { root, .. } } => {
            vec![ContentRef::Manifest { root: root.clone() }]
        }
        TimelineFact::ItemRevision { change: TimelineChange::Put { item } } => {
            item.display.validate().map_err(MergeError::InvalidInput)?;
            let mut refs = vec![item.body.clone()];
            refs.extend(item.display.owned_content());
            refs
        }
        _ => Vec::new(),
    };
    let mut sorted = BTreeMap::new();
    for value in values {
        value.validate().map_err(MergeError::InvalidInput)?;
        if let Some(old) = sorted.insert(value.id().clone(), value.clone())
            && old != value
        {
            return Err(MergeError::InvalidInput("one root identity has conflicting shape or length".into()));
        }
    }
    if sorted.len() > fastctx_types::sync::RECORD_ROOTS {
        return Err(MergeError::Limit(BudgetKind::Records));
    }
    Ok(sorted.into_values().collect())
}
fn record(content: &Content<'_>, input: &MergeInput) -> Result<SourceRecord> {
    let bytes = fastctx_types::sync::transport_codec::bounded_json(&input.event, 65536)
        .map_err(|_| MergeError::Limit(BudgetKind::PayloadBytes))?;
    let payload = content.block_identity(&bytes)?;
    let mut causes = input.causes.clone();
    self::causes(&mut causes);
    let roots = roots(&input.event.fact)?;
    if roots.iter().any(|r| r.id().domain != payload.id.domain) {
        return Err(MergeError::InvalidInput("record root crosses source scope".into()));
    }
    Ok(SourceRecord {
        sequence: input.event.evidence.record.sequence,
        operation: input.event.evidence.operation.clone(),
        kind: KIND.into(),
        schema: SCHEMA,
        payload,
        roots,
        causes,
    })
}
pub(super) fn encode_batch(
    content: &Content<'_>,
    target: &SourceStepTarget,
    after: Sequence,
    inputs: &[MergeInput],
    changes: &[Change],
    stage: bool,
) -> Result<Option<IngestFacts>> {
    let mut records = Vec::new();
    for input in inputs {
        records.push(record(content, input)?);
        if stage {
            content
                .stage_block(serde_json::to_vec(&input.event).map_err(|e| MergeError::InvalidInput(e.to_string()))?)?;
        }
    }
    for (ordinal, change) in changes.iter().enumerate() {
        let sequence = Sequence(
            after
                .0
                .checked_add(inputs.len() as u64)
                .and_then(|n| n.checked_add(ordinal as u64 + 1))
                .ok_or(MergeError::CounterExhausted)?,
        );
        let mut operation_bytes = b"fastctx.conversation.canonical-record.v1\0".to_vec();
        for field in [
            target.step().operation().as_str().as_bytes(),
            target.target().stream.as_str().as_bytes(),
            target.target().epoch.as_str().as_bytes(),
        ] {
            operation_bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
            operation_bytes.extend_from_slice(field);
        }
        operation_bytes.extend_from_slice(&(ordinal as u64).to_be_bytes());
        let operation = OperationId::from_bytes(Digest::of(&operation_bytes).bytes());
        let mut causes = change.causes.clone();
        causes.push(change.trigger.record.clone());
        let event = TimelineEvent {
            evidence: Evidence {
                record: RecordRef {
                    stream: target.target().stream.clone(),
                    epoch: target.target().epoch.clone(),
                    sequence,
                },
                operation,
                accepted: change.trigger.accepted,
            },
            fact: TimelineFact::ItemRevision { change: change.change.clone() },
        };
        let input = MergeInput { event, causes, agent_ancestors: Vec::new() };
        records.push(record(content, &input)?);
        if stage {
            content
                .stage_block(serde_json::to_vec(&input.event).map_err(|e| MergeError::InvalidInput(e.to_string()))?)?;
        }
    }
    if records.is_empty() {
        return Ok(None);
    }
    let target_data = target.target();
    let through = Sequence(after.0.checked_add(records.len() as u64).ok_or(MergeError::CounterExhausted)?);
    let batch = SourceBatch {
        format: 1,
        scope: target_data.custody.object.scope.clone(),
        stream: target_data.stream.clone(),
        producer: target_data.producer.clone(),
        epoch: target_data.epoch.clone(),
        operation: target.step().operation().clone(),
        kind: KIND.into(),
        schema: SCHEMA,
        after,
        through,
        records: records.into_boxed_slice().into_vec(),
    };
    batch.validate_shape().map_err(MergeError::InvalidInput)?;
    Ok(Some(IngestFacts::Local { batch }))
}
