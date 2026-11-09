use super::super::{
    catalog::RecordRef,
    conversation_identity::{AliasBinding, NativeId, TurnKey},
    cursor::{ByteCount, Revision, Sequence},
    identity::{Digest, HostGeneration, ObjectId, OperationId, SourceId},
    manifest::{ContentRef, ManifestRoot},
    timeline::TimelineChange,
};
use super::{AgentLink, BodyAuthority, BodySnapshot, ItemIdentity, PictureLink, RunLink};
use serde::{Deserialize, Serialize};

/// Original source evidence plus canonical observation order. Adapters reconcile
/// stale native snapshots before assigning this order; receipt time is not order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub record: RecordRef,
    pub operation: OperationId,
    pub accepted: Sequence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "proof", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlacementProof {
    NativeTurn,
    ClientId,
    ToolUseId,
    Checkpoint,
    Causal,
    /// A named historical heuristic, applied once by import, never by a read.
    Bootstrap {
        method: NativeId,
    },
}
impl PlacementProof {
    pub fn is_strong(&self) -> bool {
        !matches!(self, Self::Bootstrap { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "slot", rename_all = "snake_case", deny_unknown_fields)]
pub enum DisplaySlot {
    /// Reserved when the turn is created, even before its question arrives.
    Opening,
    Sibling,
    /// Genuine display nesting, not the host's complete causal parent chain.
    Child {
        parent: ObjectId,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub turn: ObjectId,
    pub slot: DisplaySlot,
    pub proof: PlacementProof,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub identity: ItemIdentity,
    /// Stable creation key; later body and placement revisions keep it.
    pub created: Sequence,
    pub authority: BodyAuthority,
    pub body: BodySnapshot,
    pub placement: Option<Placement>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnDeclaration {
    pub key: TurnKey,
    pub order: Sequence,
}

/// Text is appended as an immutable chunk, never by rebuilding the whole item.
/// The writer realizes the returned append plan and resumes the pure reducer
/// with its resulting ContentRef. Missing previous operations remain pending;
/// an offset differing from a matched previous operation is invalid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAppend {
    pub item: ObjectId,
    pub authority: BodyAuthority,
    pub previous: OperationId,
    pub part: crate::session::Part,
    pub offset: ByteCount,
    pub bytes: ByteCount,
    pub chunk: ContentRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixAnchor {
    pub through: RecordRef,
    pub digest: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRewritten {
    pub source: SourceId,
    pub previous: HostGeneration,
    pub generation: HostGeneration,
    pub preserved: Option<PrefixAnchor>,
    pub branch: ObjectId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
pub enum BranchChange {
    Select {
        branch: ObjectId,
        root: ManifestRoot,
        projection: Revision,
    },
    /// Only an explicit, verified host rollback can hide a retained item.
    Membership {
        item: ObjectId,
        branch: ObjectId,
        present: bool,
        rollback: Option<RecordRef>,
    },
}

/// Semantic payload carried by SourceRecord for conversation.timeline/schema 1.
/// Observed and TextAppend are retained inputs; ItemRevision is canonical output.
/// Rewriting a host file never changes ProducerEpoch or implicitly retracts items.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineFact {
    Observed { observation: Observation },
    ItemRevision { change: TimelineChange },
    TextAppend { append: TextAppend },
    HostRewritten { rewrite: HostRewritten },
    Branch { change: BranchChange },
    Alias { binding: AliasBinding, receipt: RecordRef },
    Turn { turn: TurnDeclaration },
    Placement { item: ObjectId, placement: Placement },
    Run { item: ObjectId, run: RunLink },
    Image { picture: PictureLink },
    Agent { agent: AgentLink },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineEvent {
    pub evidence: Evidence,
    pub fact: TimelineFact,
}
