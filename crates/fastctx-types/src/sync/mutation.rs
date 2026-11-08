//! Local accepted edits are pending operations, not authority-issued versions.

use super::{
    catalog::ObjectHead,
    cursor::{Count, LocalSequence, Revision},
    identity::{DeviceId, ObjectKey, OperationId},
    manifest::{BlobId, FileEntry, RelativePath},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_ATOMIC_OBJECTS: usize = 256;
pub const MAX_ATOMIC_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "base", rename_all = "snake_case")]
pub enum BaseVersion {
    Absent,
    Version { state_revision: Revision, content_version: Revision, root: Option<BlobId> },
}

/// An omitted field is untouched. Null is a value; deletion is explicit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum FieldEdit {
    Set { path: Vec<String>, value: Value },
    Delete { path: Vec<String> },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub search: String,
    pub replacement: String,
    pub all: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum FileEdit {
    Put { file: FileEntry },
    Delete { path: RelativePath },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case")]
pub enum OriginalIntent {
    Write { content: BlobId, schema: u16 },
    Edit { edits: Vec<TextEdit> },
    Fields { edits: Vec<FieldEdit> },
    Directory { edits: Vec<FileEdit> },
    Remove,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionMode {
    Online,
    Offline,
}

/// `before` can name the effective local body the editor actually saw. It may
/// include an earlier pending edit, whose operation is preserved as predecessor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedObject {
    pub key: ObjectKey,
    pub kind: String,
    pub observed_acl: Revision,
    pub base: BaseVersion,
    pub before: Option<BlobId>,
    pub candidate: Option<BlobId>,
    pub predecessor: Option<OperationId>,
    pub original: OriginalIntent,
    pub resolves: Vec<OperationId>,
}

/// One bounded same-hub transaction. The immutable logical request is retried
/// by operation ID; rebasing attempts do not change its request fingerprint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    pub operation: OperationId,
    pub origin: DeviceId,
    pub mode: SubmissionMode,
    pub objects: Vec<ProposedObject>,
}
impl Submission {
    pub fn validate_bounds(&self) -> Result<(), String> {
        if self.objects.is_empty() || self.objects.len() > MAX_ATOMIC_OBJECTS {
            return Err("an atomic submission has an invalid object count".into());
        }
        let hub = &self.objects[0].key.scope.hub;
        let mut keys = std::collections::BTreeSet::new();
        for object in &self.objects {
            if &object.key.scope.hub != hub || !keys.insert(&object.key) {
                return Err("an atomic submission must name distinct objects in one hub".into());
            }
            if let OriginalIntent::Fields { edits } = &object.original {
                let mut paths: Vec<&[String]> = edits
                    .iter()
                    .map(|edit| match edit {
                        FieldEdit::Set { path, .. } | FieldEdit::Delete { path } => path.as_slice(),
                    })
                    .collect();
                paths.sort();
                if paths.is_empty()
                    || paths.iter().any(|path| path.is_empty())
                    || paths.windows(2).any(|pair| pair[1].starts_with(pair[0]))
                {
                    return Err("field edits require nonempty, nonoverlapping paths".into());
                }
            }
        }
        if serde_json::to_vec(self).map_err(|error| error.to_string())?.len() > MAX_ATOMIC_BYTES {
            return Err("an atomic submission exceeds its metadata budget".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PendingState {
    Queued,
    Sending,
    OutcomeUnknown,
    Accepted,
    Conflict { operation: OperationId },
    Rejected { reason: String },
    Withdrawn,
}

/// Persisted as device.pending-edit, including before/candidate pins and outbox.
/// It has no locally invented authority revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMutation {
    pub local: LocalSequence,
    pub request: Submission,
    pub state: PendingState,
    pub preserve_offline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Acceptance {
    Applied,
    Merged,
    /// The candidate is retained as an authority-numbered version, but does not
    /// silently replace the selected current version.
    Branched {
        conflict: OperationId,
        alternative: Revision,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedObject {
    pub key: ObjectKey,
    pub allocated: Revision,
    pub current_version: Revision,
    pub acceptance: Acceptance,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityReceipt {
    pub operation: OperationId,
    pub objects: Vec<AcceptedObject>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum OperationOutcome {
    Accepted { receipt: AuthorityReceipt },
    Rejected { code: String, conflicts: Vec<(ObjectKey, Option<Revision>)> },
}

/// Both acceptance and definitive rejection are durable and idempotent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationReceipt {
    pub operation: OperationId,
    pub outcome: OperationOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionReceipt {
    pub operation: OperationId,
    pub local: LocalSequence,
    pub authority: Option<AuthorityReceipt>,
}

/// Readers can choose the authoritative head or the local effective view.
/// Pending overlays are ordered by local sequence and never enter versions PKs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSummary {
    pub operation: OperationId,
    pub local: LocalSequence,
    pub state: PendingState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveObject {
    pub authority: Option<ObjectHead>,
    /// A bounded tail; the complete queue is available through pending pages.
    pub pending: Vec<PendingSummary>,
    pub pending_count: Count,
    pub effective_content: Option<BlobId>,
    pub conflicts: Vec<OperationId>,
}
