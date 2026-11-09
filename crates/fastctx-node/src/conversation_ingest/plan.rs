//! Bounded plans only. The sealed source writer owns CAS, signatures, indexes,
//! dirty scheduling, private state retention and upload retirement atomically.
use super::checkpoint_bundle::BoundState;
use fastctx_hosts::claude::ingest::checkpoint::{CheckpointContext, PARSER_VERSION};
use fastctx_sync::{
    catalog::{SourceBatch, SourceRecord},
    cursor::{ByteCount, Revision, Sequence},
    identity::{Digest, HostGeneration, OperationId, SourceId},
    store::{
        Error,
        ingest::{IngestBatch, IngestFacts, IngestOperation, SourceCheckpoint, SourceTarget},
        ingest_effects::SourceEffects,
    },
};

/// All put/put_manifest calls for this step use exactly this operation. It is
/// reused after an uncertain reply. Only permanent retirement cancels uploads;
/// temporary failures retry without cancellation.
pub fn operation(source: &SourceId, generation: &HostGeneration, next: Revision) -> OperationId {
    let mut bytes = b"fastctx.claude.ingest-step.v1\0".to_vec();
    for field in [source.as_str().as_bytes(), generation.as_str().as_bytes()] {
        bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
        bytes.extend_from_slice(field);
    }
    bytes.extend_from_slice(&next.0.to_be_bytes());
    OperationId::from_bytes(Digest::of(&bytes).bytes())
}
pub struct Step {
    pub target: SourceTarget,
    pub expected: Option<SourceCheckpoint>,
    pub generation: HostGeneration,
    pub committed_offset: ByteCount,
    pub state: BoundState,
    /// Restored through revision-bound read_state and bound to expected.state.
    pub previous_state: Option<BoundState>,
    pub after: Sequence,
    pub records: Vec<SourceRecord>,
    pub effects: SourceEffects,
}
impl Step {
    pub fn prepare(self) -> Result<IngestOperation, Error> {
        let revision = Revision(
            self.expected
                .as_ref()
                .map_or(0, |p| p.revision.0)
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("checkpoint revision overflow".into()))?,
        );
        let context = CheckpointContext {
            source: &self.target.source,
            generation: &self.generation,
            revision,
            committed: self.committed_offset,
        };
        let previous = match (&self.expected, &self.previous_state) {
            (Some(outer), Some(state))
                if outer.source == self.target.source && outer.state == Some(state.content()) =>
            {
                Some((outer, state))
            }
            (Some(outer), None)
                if outer.state.is_none()
                    && outer.source == self.target.source
                    && outer.generation == self.generation =>
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
        if content.id().domain != self.target.custody.object.scope {
            return Err(Error::Invalid("checkpoint state belongs to another scope".into()));
        }
        let operation = operation(&self.target.source, &self.generation, revision);
        let kind = self.target.custody.kind.registration();
        let mut through = self.after;
        for record in &self.records {
            through.0 = through.0.checked_add(1).ok_or_else(|| Error::Invalid("source sequence overflow".into()))?;
            if record.sequence != through || record.kind != kind.id || record.schema != kind.schema {
                return Err(Error::Invalid("source records do not continue their registered stream".into()));
            }
        }
        let facts = if self.records.is_empty() {
            None
        } else {
            Some(IngestFacts::Local {
                batch: SourceBatch {
                    format: 1,
                    scope: self.target.custody.object.scope.clone(),
                    stream: self.target.stream.clone(),
                    producer: self.target.producer.clone(),
                    epoch: self.target.epoch.clone(),
                    kind: kind.id.into(),
                    schema: kind.schema,
                    after: self.after,
                    through,
                    records: self.records,
                },
            })
        };
        let next = SourceCheckpoint {
            source: self.target.source.clone(),
            generation: self.generation,
            revision,
            committed_offset: self.committed_offset,
            parser_version: PARSER_VERSION,
            state: Some(content),
        };
        IngestOperation::new(
            operation,
            vec![IngestBatch { target: self.target, expected: self.expected, next, facts, effects: self.effects }],
        )
    }
}
