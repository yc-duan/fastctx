//! Bounded plans built from engine-issued source steps. The writer alone owns
//! checkpoint CAS, signatures, indexes, root lifetimes and upload retirement.
use super::checkpoint_bundle::BoundState;
use fastctx_hosts::claude::ingest::checkpoint::{CheckpointContext, PARSER_VERSION};
use fastctx_sync::{
    catalog::{SourceBatch, SourceRecord},
    cursor::{ByteCount, Count, Sequence},
    identity::{Digest, OperationId},
    store::{
        Error,
        ingest::{IngestBatch, IngestFacts, IngestOperation, SourceCheckpoint},
        ingest_effects::SourceEffects,
        source_step::{SourceStep, SourceStepTarget},
    },
};

/// One logical record's operation within an issued step. A fan-out keeps the same
/// logical positions in each destination. Dropping destinations does not renumber
/// records, and a retry keeps the issued step instead of inventing a new batch ID.
pub fn record_operation(step: &SourceStep, position: Count) -> OperationId {
    let mut bytes = Vec::with_capacity(160);
    let position = position.0.to_be_bytes();
    for field in [b"fastctx.conversation.source-record.v1".as_slice(), step.operation().as_str().as_bytes(), &position]
    {
        bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
        bytes.extend_from_slice(field);
    }
    OperationId::from_bytes(Digest::of(&bytes).bytes())
}

pub struct Destination {
    pub target: SourceStepTarget,
    pub after: Sequence,
    /// Already stored payloads and their direct owned roots. Descriptor Metadata
    /// pages carry part ownership; logical cross-object links are not roots here.
    pub records: Vec<SourceRecord>,
    pub effects: SourceEffects,
}
impl Destination {
    fn prepare(self, step: &SourceStep) -> Result<IngestBatch, Error> {
        if self.target.step().key() != step.key() || self.target.step().operation() != step.operation() {
            return Err(Error::Invalid("destination belongs to another source step".into()));
        }
        if self.records.len() > 256 {
            return Err(Error::Invalid("source batch exceeds its record count".into()));
        }
        let target = self.target.target();
        let kind = target.custody.kind.registration();
        let mut through = self.after;
        for (position, record) in self.records.iter().enumerate() {
            through.0 = through.0.checked_add(1).ok_or_else(|| Error::Invalid("source sequence overflow".into()))?;
            if record.sequence != through
                || record.kind != kind.id
                || record.schema != kind.schema
                || record.operation != record_operation(step, Count(position as u64))
            {
                return Err(Error::Invalid("source record differs from its stream or step position".into()));
            }
        }
        let facts = if self.records.is_empty() {
            None
        } else {
            Some(IngestFacts::Local {
                batch: SourceBatch {
                    format: 1,
                    scope: target.custody.object.scope.clone(),
                    stream: target.stream.clone(),
                    producer: target.producer.clone(),
                    epoch: target.epoch.clone(),
                    operation: step.operation().clone(),
                    kind: kind.id.into(),
                    schema: kind.schema,
                    after: self.after,
                    through,
                    records: self.records,
                },
            })
        };
        Ok(IngestBatch { target: self.target, facts, effects: self.effects })
    }
}

/// Exactly one parser checkpoint, owned by the primary source of `step`.
/// Destinations hold only their facts and effects, never copied checkpoints.
pub struct Step {
    pub step: SourceStep,
    pub expected: Option<SourceCheckpoint>,
    pub committed_offset: ByteCount,
    pub state: BoundState,
    /// Restored through revision-bound read_state and bound to expected.state.
    pub previous_state: Option<BoundState>,
    pub destinations: Vec<Destination>,
}
impl Step {
    pub fn prepare(self) -> Result<IngestOperation, Error> {
        if self.destinations.is_empty() || self.destinations.len() > 256 {
            return Err(Error::Invalid("source step needs a bounded nonempty destination set".into()));
        }
        if self.expected.as_ref().map(|p| p.revision) != self.step.expected() {
            return Err(Error::Invalid("parser checkpoint differs from its issued source step".into()));
        }
        let primary = self.step.primary();
        let context = CheckpointContext {
            source: &primary.source,
            generation: self.step.generation(),
            revision: self.step.revision(),
            committed: self.committed_offset,
        };
        let previous = match (&self.expected, &self.previous_state) {
            (Some(outer), Some(state)) if outer.source == primary.source && outer.state == Some(state.content()) => {
                Some((outer, state))
            }
            (Some(outer), None)
                if outer.state.is_none()
                    && outer.source == primary.source
                    && &outer.generation == self.step.generation() =>
            {
                None
            }
            (None, None) => None,
            _ => return Err(Error::Invalid("previous parser state differs from its expected checkpoint".into())),
        };
        let old_context = previous.map(|(outer, _)| CheckpointContext {
            source: &outer.source,
            generation: &outer.generation,
            revision: outer.revision,
            committed: outer.committed_offset,
        });
        let old = previous.zip(old_context.as_ref()).map(|((_, state), context)| (state.checkpoint(), context));
        self.state.checkpoint().validate_transition(&context, old).map_err(|e| Error::Invalid(e.to_string()))?;
        let content = self.state.content();
        if content.id().domain != primary.custody.object.scope {
            return Err(Error::Invalid("checkpoint state belongs to another scope".into()));
        }
        let next = SourceCheckpoint {
            source: primary.source.clone(),
            generation: self.step.generation().clone(),
            revision: self.step.revision(),
            committed_offset: self.committed_offset,
            parser_version: PARSER_VERSION,
            state: Some(content),
        };
        let batches = self
            .destinations
            .into_iter()
            .map(|destination| destination.prepare(&self.step))
            .collect::<Result<_, _>>()?;
        IngestOperation::new(self.step, self.expected, next, batches)
    }
}
