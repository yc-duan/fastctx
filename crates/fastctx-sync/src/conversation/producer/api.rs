use super::content::Publication;
use crate::{
    catalog::RecordRef,
    conversation::{self, body_tree::BodyTreeError},
    cursor::Sequence,
    identity::{HostGeneration, ObjectId, SourceId},
    manifest::{ContentBlock, ManifestReply, ManifestRoot},
    store::{
        self,
        ingest::{IngestFacts, SourceIngest},
        ingest_effects::SourceEffects,
        source_step::SourceStepTarget,
    },
};
use fastctx_types::sync::conversation_timeline::TimelineEvent;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeInput {
    pub event: TimelineEvent,
    /// Causes for a new record. Exact event replays retain the committed causes.
    pub causes: Vec<RecordRef>,
    /// A verified complete parent chain for Agent facts; empty for other facts.
    pub agent_ancestors: Vec<ObjectId>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSeed {
    pub source: SourceId,
    pub generation: HostGeneration,
}
#[derive(Clone, Debug)]
pub struct InitialState {
    pub active_branch: ObjectId,
    pub sources: Vec<SourceSeed>,
}

/// Current-step content is private to its step and cannot be read from the store.
pub trait SuppliedContent: Send + Sync {
    fn block(&self, block: &ContentBlock) -> Option<&[u8]>;
    fn manifest(&self, root: &ManifestRoot) -> Option<&ManifestReply>;
}
impl SuppliedContent for () {
    fn block(&self, _: &ContentBlock) -> Option<&[u8]> {
        None
    }
    fn manifest(&self, _: &ManifestRoot) -> Option<&ManifestReply> {
        None
    }
}

#[derive(Clone, Debug)]
pub struct MergeBudget {
    pub max_effects: usize,
    pub max_effect_bytes: usize,
    pub max_records: usize,
    pub max_payload_bytes: u64,
    /// Remaining operation metadata after checkpoints and destination overhead.
    pub max_commit_bytes: usize,
    pub max_work_bytes: usize,
    pub max_work_units: u32,
    pub max_content_reads: u32,
}
impl Default for MergeBudget {
    fn default() -> Self {
        Self {
            max_effects: 256,
            max_effect_bytes: 262144,
            max_records: 256,
            max_payload_bytes: crate::store::ingest::PAYLOAD_BYTES,
            max_commit_bytes: 240 * 1024,
            max_work_bytes: 8 * 1024 * 1024,
            max_work_units: 128,
            max_content_reads: 2048,
        }
    }
}
pub struct MergeRequest<'a> {
    pub target: &'a SourceStepTarget,
    pub conversation: &'a ObjectId,
    pub after: Sequence,
    pub inputs: &'a [MergeInput],
    pub supplied: &'a dyn SuppliedContent,
    pub initial: Option<&'a InitialState>,
    pub other_effects: SourceEffects,
    pub budget: MergeBudget,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetKind {
    Effects,
    EffectBytes,
    Records,
    PayloadBytes,
    CommitBytes,
    WorkBytes,
    WorkUnits,
    ContentReads,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    Complete,
    Budget(BudgetKind),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeProgress {
    pub consumed_inputs: usize,
    pub appended_inputs: usize,
    pub replayed_inputs: usize,
    pub ready_work: bool,
    pub waiting_dependencies: bool,
    pub stop: StopReason,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeUsage {
    pub effects: usize,
    pub effect_bytes: usize,
    pub records: usize,
    pub payload_bytes: u64,
    pub commit_bytes: usize,
    pub work_bytes: usize,
    pub work_units: u32,
    pub content_reads: u32,
}
#[derive(Debug)]
pub enum MergeError {
    Reduction {
        evidence: Box<fastctx_types::sync::conversation_timeline::Evidence>,
        error: conversation::Error,
    },
    Store(store::Error),
    InvalidInput(String),
    CorruptState {
        key: Vec<u8>,
        reason: String,
    },
    Body(BodyTreeError),
    UnitTooLarge {
        needed: Box<MergeUsage>,
        available: Box<MergeBudget>,
        limit: BudgetKind,
    },
    CounterExhausted,
    #[doc(hidden)]
    Limit(BudgetKind),
}
impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "canonical producer: {self:?}")
    }
}
impl std::error::Error for MergeError {}
impl From<store::Error> for MergeError {
    fn from(e: store::Error) -> Self {
        Self::Store(e)
    }
}
impl From<BodyTreeError> for MergeError {
    fn from(e: BodyTreeError) -> Self {
        Self::Body(e)
    }
}
pub(super) type Result<T> = std::result::Result<T, MergeError>;

pub struct PreparedMerge {
    pub(super) target: SourceStepTarget,
    pub(super) publications: Vec<Publication>,
    pub(super) output: MergeOutput,
}
impl PreparedMerge {
    pub fn progress(&self) -> &MergeProgress {
        &self.output.progress
    }
    pub fn usage(&self) -> &MergeUsage {
        &self.output.usage
    }
    /// Publishes only this issued step's immutable bytes. The caller still owns
    /// the single ingest transaction and must not commit a different checkpoint.
    pub async fn publish(self, source: &dyn SourceIngest) -> Result<MergeOutput> {
        for publication in self.publications {
            match publication {
                Publication::Block { block, bytes } => {
                    let encoded =
                        crate::blocks::codec::encode(block.id.domain.clone(), bytes, crate::manifest::Encoding::Raw)?;
                    if encoded.representation.content != block {
                        return Err(MergeError::InvalidInput("planned block identity changed".into()));
                    }
                    source.put_block(self.target.clone(), encoded.representation, encoded.bytes).await?;
                }
                Publication::Manifest(page) => {
                    source.put_manifest(self.target.clone(), page).await?;
                }
            }
        }
        Ok(self.output)
    }
}
pub struct MergeOutput {
    pub progress: MergeProgress,
    pub usage: MergeUsage,
    pub facts: Option<IngestFacts>,
    pub effects: SourceEffects,
}
