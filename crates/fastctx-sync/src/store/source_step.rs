//! Engine-issued ingestion steps. A step binds one source attempt's uploads,
//! destinations and commit to an operation derived here, never by an adapter.
use super::ingest::{IngestReceipt, SourceTarget};
use crate::{
    cursor::Revision,
    identity::{Digest, HostGeneration, OperationId, ScopeKey, SourceId},
};

/// The persisted coordinates of one step: its primary source and issued revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepKey {
    pub scope: ScopeKey,
    pub source: SourceId,
    pub revision: Revision,
}

/// Issued by `begin_step`. Reopening the same active step returns the same
/// operation; a cancelled step's revision is never issued again.
#[derive(Clone)]
pub struct SourceStep {
    primary: SourceTarget,
    generation: HostGeneration,
    expected: Option<Revision>,
    revision: Revision,
    operation: OperationId,
}
impl SourceStep {
    pub(crate) fn new(
        primary: SourceTarget,
        generation: HostGeneration,
        expected: Option<Revision>,
        revision: Revision,
    ) -> Self {
        let operation = step_operation(&primary.custody.object.scope, &primary.source, &generation, revision);
        Self { primary, generation, expected, revision, operation }
    }
    pub fn primary(&self) -> &SourceTarget {
        &self.primary
    }
    pub fn generation(&self) -> &HostGeneration {
        &self.generation
    }
    /// The committed checkpoint revision this step builds on.
    pub fn expected(&self) -> Option<Revision> {
        self.expected
    }
    /// The checkpoint revision a successful commit installs.
    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn operation(&self) -> &OperationId {
        &self.operation
    }
    pub fn key(&self) -> StepKey {
        StepKey {
            scope: self.primary.custody.object.scope.clone(),
            source: self.primary.source.clone(),
            revision: self.revision,
        }
    }
}

/// One destination bound to a step. Uploads for that destination go through it.
#[derive(Clone)]
pub struct SourceStepTarget {
    step: SourceStep,
    target: SourceTarget,
}
impl SourceStepTarget {
    pub(crate) fn new(step: SourceStep, target: SourceTarget) -> Self {
        Self { step, target }
    }
    pub fn step(&self) -> &SourceStep {
        &self.step
    }
    pub fn target(&self) -> &SourceTarget {
        &self.target
    }
}

#[derive(Clone, Debug)]
pub enum CancelProgress {
    /// The step is durably cancelled; this many staged pins were released now.
    Draining { released: u32 },
    /// Cancelled and every staged pin released.
    Done,
    /// The step had already committed; its original receipt stands.
    Committed { receipt: Box<IngestReceipt> },
}

/// Length-prefixed fields under a fixed domain, so no two coordinates collide.
pub(crate) fn step_operation(
    scope: &ScopeKey,
    source: &SourceId,
    generation: &HostGeneration,
    revision: Revision,
) -> OperationId {
    let mut bytes = b"fastctx-sync/source-step/v1\0".to_vec();
    let hub = scope.hub.to_string();
    for field in [hub.as_str(), scope.scope.as_str(), source.as_str(), generation.as_str()] {
        bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
        bytes.extend_from_slice(field.as_bytes());
    }
    bytes.extend_from_slice(&revision.0.to_be_bytes());
    OperationId::from_bytes(Digest::of(&bytes).bytes())
}
