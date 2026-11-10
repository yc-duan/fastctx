//! Object heads, source facts and catalog pages carry separate revisions.

use super::{
    cursor::{Completeness, Revision, ScopeCursor, Sequence, SnapshotFence, VersionRef},
    identity::{DeviceId, Digest, ObjectId, ObjectKey, OperationId, Producer, ProducerEpoch, ScopeKey, StreamId},
    manifest::{ContentBlock, ContentRef, ManifestRoot},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    Asset,
    Runtime,
    Derived,
    Scratch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Authority {
    Hub,
    Device { device: DeviceId, epoch: ProducerEpoch },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Live,
    Removed,
    Deleted,
}

/// Every head names its verification path. Stream discovery remains pending
/// until the original batches reach this authenticated epoch and prefix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "evidence", rename_all = "snake_case")]
pub enum HeadEvidence {
    Accepted { proof: Digest },
    Stream { epoch: Digest, through: Sequence, prefix_digest: Digest },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectHead {
    pub key: ObjectKey,
    pub kind: String,
    pub schema: u16,
    pub acl_revision: Revision,
    pub authority: Authority,
    pub evidence: HeadEvidence,
    /// Latest authority-allocated state revision, including retained conflicts.
    pub revision: VersionRef,
    /// The selected body version; it need not be the largest allocated revision.
    pub current_version: VersionRef,
    pub content: Option<ContentRef>,
    /// All retained versions remain discoverable after catalog feed compaction.
    pub history: Option<ManifestRoot>,
    pub lifecycle: Lifecycle,
    pub completeness: Completeness,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogPage {
    pub fence: SnapshotFence,
    pub objects: Vec<ObjectHead>,
    /// Original acceptance fragments, unique by proof ID and included in the page budget.
    pub proofs: Vec<super::proof::SignedAcceptanceFragment>,
    pub next: Option<ObjectId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogChange {
    pub sequence: Sequence,
    pub operation: OperationId,
    pub head: ObjectHead,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum FeedEntry {
    Changed {
        change: Box<CatalogChange>,
        prefix_digest: super::identity::Digest,
    },
    /// Explicitly covers excluded sequence numbers under exactly this view.
    Skipped {
        after: Sequence,
        through: Sequence,
        view: super::identity::ViewId,
        /// The original feed anchor, independent of excluded kinds and schemas.
        prefix_digest: super::identity::Digest,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesPage {
    pub after: ScopeCursor,
    pub through: ScopeCursor,
    pub entries: Vec<FeedEntry>,
    pub proofs: Vec<super::proof::SignedAcceptanceFragment>,
    pub more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub sequence: Sequence,
    pub operation: OperationId,
    pub kind: String,
    pub schema: u16,
    pub payload: ContentBlock,
    /// Every root the payload refers to directly. A root's own closure carries
    /// what it refers to in turn: a descriptor that names further content is a
    /// metadata page whose codec references it exactly. Holders retain and fetch
    /// the closures of exactly these roots; the kind's validator rejects a
    /// payload whose direct references differ from them.
    pub roots: Vec<ContentRef>,
    pub causes: Vec<RecordRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordRef {
    pub stream: StreamId,
    pub epoch: ProducerEpoch,
    pub sequence: Sequence,
}

/// Authenticated together with its complete source binding. A relay cannot
/// replace the scope, producer or sequence range of a signed batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBatch {
    pub format: u16,
    pub scope: ScopeKey,
    pub stream: StreamId,
    pub producer: Producer,
    pub epoch: ProducerEpoch,
    /// Idempotency key of this signed batch: the ingestion step's operation for
    /// its first part, a derived operation for later parts, or the single fact's
    /// operation. Each record keeps its own logical operation across batch cuts.
    pub operation: OperationId,
    /// A stream is bound to one registered kind/schema for its whole epoch.
    pub kind: String,
    pub schema: u16,
    pub after: Sequence,
    pub through: Sequence,
    pub records: Vec<SourceRecord>,
}
impl SourceBatch {
    /// The range and record binding every holder checks before trusting a batch,
    /// whether it signs it, verifies it or forwards it. Registry rules are separate.
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.format != 1
            || self.records.is_empty()
            || self.through.0.checked_sub(self.after.0) != Some(self.records.len() as u64)
        {
            return Err("invalid source batch format or range".into());
        }
        let mut operations = std::collections::BTreeSet::new();
        for (index, record) in self.records.iter().enumerate() {
            if record.sequence.0 != self.after.0 + index as u64 + 1
                || record.kind != self.kind
                || record.schema != self.schema
                || record.payload.id.domain != self.scope
                || record.payload.decoded_bytes > super::BLOCK_BYTES
                || !operations.insert(&record.operation)
            {
                return Err("source record differs from its batch range binding".into());
            }
            if record.roots.len() > super::RECORD_ROOTS {
                return Err("source record owns too many content roots".into());
            }
            let mut roots = std::collections::BTreeSet::new();
            for root in &record.roots {
                root.validate()?;
                if root.id().domain != self.scope || !roots.insert(root.id()) {
                    return Err("source record roots must be distinct and inside its scope".into());
                }
            }
        }
        Ok(())
    }
}
