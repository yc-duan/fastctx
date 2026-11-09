//! Local parser progress. File identity, mutation observations, host rewrites and
//! source sequence are separate coordinates; no checkpoint is an asset deletion.
use super::{
    document::Completion,
    jsonl::{PartialLine, Row},
};
use fastctx_types::{
    file_identity::{FileChangeStamp, FileIdentity},
    sync::{
        cursor::{ByteCount, Revision},
        identity::{Digest, HostGeneration, ObjectId, SourceId},
        manifest::ContentRef,
    },
};
use serde::{Deserialize, Serialize};

mod read_ahead;
mod validate;
pub use read_ahead::ReadAhead;
pub use validate::{BLOCKED_REASON_BYTES, CheckpointContext};

pub const PARSER_VERSION: u16 = 1;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeCheckpointV1 {
    pub version: u16,
    pub file_identity: FileIdentity,
    pub observed_stamp: FileChangeStamp,
    /// Following: parsed bytes in this generation; read-ahead may hold more.
    /// Reconciling: the new file's
    /// contiguous audit position; the outer committed offset still belongs to
    /// the old generation until the final consistent checkpoint installs the new.
    pub read_offset: ByteCount,
    pub partial: Option<Partial>,
    /// Unparsed bytes from an already-read window. Keep the immutable whole block
    /// and advance only its cursor, so a short row never causes a host reread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_ahead: Option<ReadAhead>,
    /// None when anchors are stored in the registered HostRows family instead.
    pub anchors_root: Option<ContentRef>,
    /// None when native-row lookups use the registered HostRows family instead.
    pub row_index_root: Option<ContentRef>,
    pub phase: Phase,
    pub audit_cursor: AuditCursor,
}
/// A validated large row may need several bounded semantic transactions. Keep
/// its private tree and consumer position until all of those facts are committed;
/// restarting never reparses already validated host bytes to recover that work.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case", deny_unknown_fields)]
pub enum Partial {
    Reading { line: PartialLine },
    Publishing { source: SourceId, generation: HostGeneration, row: Row, consumer: Option<ContentRef> },
}
impl Partial {
    pub fn coordinates(&self) -> (&SourceId, &HostGeneration) {
        match self {
            Self::Reading { line } => (&line.document.key.source, &line.document.key.generation),
            Self::Publishing { source, generation, .. } => (source, generation),
        }
    }
    fn boundaries(&self) -> (ByteCount, ByteCount) {
        match self {
            Self::Reading { line } => (line.document.key.line_start, line.read_offset),
            Self::Publishing { row, .. } => (row.start, row.end),
        }
    }
    fn roots(&self) -> Vec<&ContentRef> {
        match self {
            Self::Reading { line } => line.document.bodies.roots().to_vec(),
            Self::Publishing { row, consumer, .. } => {
                let bodies = match &row.content {
                    Completion::Valid(document) => &document.bodies,
                    Completion::Invalid { bodies, .. } => bodies,
                };
                let mut roots = bodies.roots().to_vec();
                roots.extend(consumer.iter());
                roots
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditCursor {
    pub observation: FileChangeStamp,
    pub through: ByteCount,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum Phase {
    Following,
    Reconciling {
        previous: Revision,
        generation: HostGeneration,
        reason: RewriteReason,
        prefix: Prefix,
        old_branch: ObjectId,
        new_branch: ObjectId,
    },
    SourceMissing,
    Blocked {
        reason: String,
        dependency: Option<ObjectId>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteReason {
    Truncated,
    Replaced,
    AnchorChanged,
    HostRewritten,
    AuditChanged,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub offset: ByteCount,
    pub bytes: u32,
    pub digest: Digest,
    /// Last fully verified row boundary at or before this anchor.
    pub row_boundary: ByteCount,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefix {
    pub checked_through: ByteCount,
    pub row_boundary: ByteCount,
    pub anchor: Option<Anchor>,
}
impl ClaudeCheckpointV1 {
    /// All returned roots must be explicit edges of the checkpoint-state bundle.
    pub fn roots(&self) -> Vec<&ContentRef> {
        let mut roots = Vec::with_capacity(7);
        roots.extend(self.anchors_root.iter());
        roots.extend(self.row_index_root.iter());
        roots.extend(self.read_ahead.iter().map(|ahead| &ahead.content));
        if let Some(partial) = &self.partial {
            roots.extend(partial.roots());
        }
        roots
    }
}
