//! Alias coordinates and replay checks do not infer or authenticate evidence.

use super::{IdentityError, ItemKey, ItemRole, ProvisionalKey, RelationKey, RelationRole};
use crate::sync::identity::{Digest, ObjectId, OperationId};
use serde::{Deserialize, Serialize};

/// An observation which may acquire a proven final native identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "key", rename_all = "snake_case", deny_unknown_fields)]
pub enum AliasSource {
    Provisional(ProvisionalKey),
    Legacy(ItemKey),
}

impl AliasSource {
    pub fn id(&self) -> ObjectId {
        match self {
            Self::Provisional(key) => key.id(),
            Self::Legacy(key) => key.id(),
        }
    }

    fn conversation(&self) -> &ObjectId {
        match self {
            Self::Provisional(key) => &key.conversation,
            Self::Legacy(key) => &key.conversation,
        }
    }

    fn role(&self) -> ItemRole {
        match self {
            Self::Provisional(key) => key.role,
            Self::Legacy(key) => key.role,
        }
    }
}

/// A checked shape for an alias revision. Committing it still requires the
/// host receipt or other authoritative evidence; matching text is insufficient.
/// Applying it retains the source row's display position, never renames old
/// facts, and does not allow a provisional arrival to determine the final ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AliasFields", into = "AliasFields")]
pub struct AliasBinding {
    source: AliasSource,
    target: ItemKey,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AliasFields {
    source: AliasSource,
    target: ItemKey,
}

impl AliasBinding {
    pub fn new(source: AliasSource, target: ItemKey) -> Result<Self, IdentityError> {
        if matches!(&source, AliasSource::Legacy(key) if !key.native.is_legacy()) {
            return Err(IdentityError::AliasSource);
        }
        if target.native.is_legacy() {
            return Err(IdentityError::AliasTarget);
        }
        if source.conversation() != &target.conversation {
            return Err(IdentityError::AliasConversation);
        }
        if source.role() != target.role {
            return Err(IdentityError::AliasRole);
        }
        Ok(Self { source, target })
    }

    pub fn source(&self) -> &AliasSource {
        &self.source
    }

    pub fn target(&self) -> &ItemKey {
        &self.target
    }

    pub fn relation(&self) -> RelationKey {
        RelationKey {
            conversation: self.target.conversation.clone(),
            role: RelationRole::Alias,
            from: self.source.id(),
            to: self.target.id(),
        }
    }
}

impl TryFrom<AliasFields> for AliasBinding {
    type Error = IdentityError;

    fn try_from(value: AliasFields) -> Result<Self, Self::Error> {
        Self::new(value.source, value.target)
    }
}

impl From<AliasBinding> for AliasFields {
    fn from(value: AliasBinding) -> Self {
        Self { source: value.source, target: value.target }
    }
}

/// A single immutable observation, not the item's latest mutable body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEvidence {
    pub operation: OperationId,
    pub item: ObjectId,
    pub body: Digest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayMatch {
    Same,
    DistinctOperation,
}

impl ReplayEvidence {
    /// Only identical evidence for the same operation may be coalesced. A new
    /// operation remains new even when a body changes A -> B -> A. This does
    /// not select a revision or decide whether the producer may publish it.
    pub fn compare(&self, other: &Self) -> Result<ReplayMatch, IdentityError> {
        if self.operation != other.operation {
            return Ok(ReplayMatch::DistinctOperation);
        }
        if self.item != other.item {
            return Err(IdentityError::ReplayItem);
        }
        if self.body != other.body {
            return Err(IdentityError::ReplayBody);
        }
        Ok(ReplayMatch::Same)
    }
}
