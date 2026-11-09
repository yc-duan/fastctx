//! Source inputs carry their own canonical evidence. Typed descriptor Metadata
//! pages retain nested parts; logical object and record links never grant custody.
use super::plan::record_operation;
use fastctx_sync::{
    conversation::source::event,
    store::{Error, source_step::SourceStepTarget},
};
use fastctx_types::sync::{
    catalog::{RecordRef, SourceRecord},
    conversation_timeline::{Evidence, KIND, SCHEMA, TimelineEvent, TimelineFact},
    cursor::{Count, Sequence},
    identity::{Digest, OperationId},
    manifest::{ContentBlock, ContentRef},
};

pub struct PreparedFact {
    step: OperationId,
    event: TimelineEvent,
    payload: Vec<u8>,
    roots: Vec<ContentRef>,
}
impl PreparedFact {
    /// Inputs get stable per-record operations before the canonical merger sees
    /// them. Canonical output records are returned by that merger unchanged.
    pub fn input(
        target: &SourceStepTarget,
        position: Count,
        sequence: Sequence,
        accepted: Sequence,
        fact: TimelineFact,
    ) -> Result<Self, Error> {
        if matches!(fact, TimelineFact::ItemRevision { .. }) {
            return Err(Error::Invalid("canonical revisions are produced by the shared merger".into()));
        }
        let event = TimelineEvent {
            evidence: Evidence {
                record: RecordRef {
                    stream: target.target().stream.clone(),
                    epoch: target.target().epoch.clone(),
                    sequence,
                },
                operation: record_operation(target.step(), position),
                accepted,
            },
            fact,
        };
        let roots =
            fastctx_sync::conversation::source::direct_roots(&target.target().custody.object.scope, &event.fact)
                .map_err(Error::Invalid)?;
        let payload = event::encode(&event).map_err(Error::Invalid)?;
        Ok(Self { step: target.step().operation().clone(), event, payload, roots })
    }
    pub fn event(&self) -> &TimelineEvent {
        &self.event
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    pub fn roots(&self) -> &[ContentRef] {
        &self.roots
    }
    /// Finish after this exact event was uploaded through its bound source step.
    pub fn record(
        self,
        target: &SourceStepTarget,
        payload: ContentBlock,
        causes: Vec<RecordRef>,
    ) -> Result<SourceRecord, Error> {
        let scope = &target.target().custody.object.scope;
        let kind = target.target().custody.kind.registration();
        if self.step != *target.step().operation()
            || kind.id != KIND
            || kind.schema != SCHEMA
            || self.event.evidence.record.stream != target.target().stream
            || self.event.evidence.record.epoch != target.target().epoch
            || &payload.id.domain != scope
            || payload.id.digest != Digest::of(&self.payload)
            || payload.decoded_bytes as usize != self.payload.len()
        {
            return Err(Error::Invalid("timeline event differs from its source or uploaded payload".into()));
        }
        Ok(SourceRecord {
            sequence: self.event.evidence.record.sequence,
            operation: self.event.evidence.operation,
            kind: KIND.into(),
            schema: SCHEMA,
            payload,
            roots: self.roots,
            causes,
        })
    }
}
