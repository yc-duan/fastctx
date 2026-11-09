//! Durable progress, distinct from transport delivery and volatile UI updates.

use super::identity::{
    AuthorityEpoch, DeviceId, Digest, FeedEpoch, ObjectId, ProducerEpoch, ScopeKey, StreamId, ViewId,
};
use serde::{Deserialize, Serialize};

macro_rules! counter {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(pub u64);
        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(text: String) -> Result<Self, Self::Error> {
                let value = text.parse::<u64>().map_err(|_| concat!("invalid ", stringify!($name)).to_string())?;
                if value.to_string() != text {
                    return Err("a counter must use canonical decimal notation".into());
                }
                Ok(Self(value))
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0.to_string()
            }
        }
    };
}

counter!(Sequence, "A protocol position, encoded as a decimal string to preserve all 64 bits in JavaScript.");
counter!(Revision, "An object or authorization revision; only its authority advances it.");
counter!(LocalSequence, "One database's change cursor, never a hub-assigned scope sequence.");
counter!(ByteCount, "A byte count that remains exact across the GUI boundary.");
counter!(Count, "An exact cardinality or index, distinct from a byte offset.");

/// Version identities are comparable for equality, not ordered across epochs.
/// Only ObjectHead selects the current version. A new authority epoch alone
/// never invalidates a feed cursor.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VersionRef {
    pub authority_epoch: AuthorityEpoch,
    pub revision: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeCursor {
    pub scope: ScopeKey,
    pub epoch: FeedEpoch,
    pub acl_revision: Revision,
    /// Missing on an older cursor means that reconciliation is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewId>,
    #[serde(alias = "applied")]
    pub through: Sequence,
    /// The authority feed prefix at `through`; absence requires reconciliation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_digest: Option<Digest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamCursor {
    pub scope: ScopeKey,
    pub stream: StreamId,
    pub epoch: ProducerEpoch,
    /// The last contiguous committed record, not the largest number observed.
    pub committed: Sequence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotFence {
    pub cursor: ScopeCursor,
    pub root: super::manifest::ManifestRoot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageCursor {
    pub snapshot: SnapshotFence,
    pub after: Option<ObjectId>,
}

/// A half-open range of storage block indexes, never byte offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRange {
    pub start: Count,
    pub end: Count,
}

impl BlockRange {
    pub fn validate(&self) -> Result<(), String> {
        if self.start >= self.end { Err("a range must be nonempty and increasing".into()) } else { Ok(()) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "holder", rename_all = "snake_case")]
pub enum Holder {
    Hub { hub: super::identity::HubId },
    Device { device: DeviceId },
}

/// One replica's current right to retain a scope. Regrant creates a new
/// publication epoch; neither old queues nor old ACKs complete the new tenure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldingPeriod {
    pub holder: Holder,
    pub scope: ScopeKey,
    pub acl_revision: Revision,
    pub publication_epoch: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockProgress {
    pub manifest: super::manifest::ManifestRoot,
    /// Verified, durable block indexes; sorted, disjoint half-open ranges.
    pub verified: Vec<BlockRange>,
    pub total_blocks: Count,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Completeness {
    MetadataOnly,
    Pending { missing_records: Count, missing_bytes: ByteCount },
    Complete { root: super::manifest::ContentRef },
    Unavailable { reason: String },
    LegacyIncomplete { source: super::identity::DeviceId, reason: String },
}

/// Sent only after FULL commit. A catalog ACK does not acknowledge referenced
/// bodies; a stream ACK does not claim that all of its images are downloaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DurableAck {
    Catalog {
        holding: HoldingPeriod,
        cursor: ScopeCursor,
    },
    Stream {
        holding: HoldingPeriod,
        cursor: StreamCursor,
    },
    Blocks {
        holding: HoldingPeriod,
        progress: BlockProgress,
    },
    /// One verified decoded block, durably published with its index and pin.
    /// This does not acknowledge the object's other blocks or its source tail.
    Block {
        holding: HoldingPeriod,
        object: super::identity::ObjectKey,
        #[serde(rename = "object_kind")]
        kind: String,
        schema: u16,
        content: super::manifest::ContentBlock,
    },
}
