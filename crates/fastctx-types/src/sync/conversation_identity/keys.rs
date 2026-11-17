//! Content-independent keys. Deserialization rejects accidental branch/body inputs.

use super::{NativeAgentKey, NativeId, NativeItemKey, NativeTurnKey, encoding::Encoder};
use crate::{
    ResidentId,
    sync::identity::{DeviceId, HubId, ObjectId, ScopeId, SourceId},
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Verified immutable birth coordinates. The opener and later owners are
/// properties, not identity. A real fork must supply its own birth coordinates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationKey {
    Birth { hub: HubId, device: DeviceId, resident: ResidentId, born_at: Timestamp },
    NativeChild { parent: ObjectId, agent: NativeAgentKey },
}

impl ConversationKey {
    /// The canonical conversation and its metadata object have this identity.
    pub fn id(&self) -> ObjectId {
        let mut into = Encoder::new("conversation");
        match self {
            Self::Birth { hub, device, resident, born_at } => {
                into.text("birth");
                into.text(hub.as_str());
                into.text(device.as_str());
                into.text(resident.as_str());
                into.field(&born_at.as_nanosecond().to_be_bytes());
            }
            Self::NativeChild { parent, agent } => {
                into.text("native-child");
                into.text(parent.as_str());
                agent.encode(&mut into);
            }
        }
        ObjectId::from_bytes(into.finish())
    }

    /// A stable conversation scope coordinate, not an authorization descriptor.
    pub fn scope(&self) -> ScopeId {
        conversation_scope(&self.id())
    }

    pub fn object(&self, kind: ConversationObject) -> ObjectId {
        kind.id(&self.id())
    }
}

/// Derives the scope after the canonical conversation has already been resolved.
/// An ACL/ScopeDescriptor must still establish the caller's rights to it.
pub fn conversation_scope(conversation: &ObjectId) -> ScopeId {
    let mut into = Encoder::new("scope");
    into.text(conversation.as_str());
    ScopeId::from_bytes(into.finish())
}

/// Separate registered objects must not reuse the metadata object's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationObject {
    Metadata,
    Timeline,
    Agents,
}

impl ConversationObject {
    pub fn id(self, conversation: &ObjectId) -> ObjectId {
        if self == Self::Metadata {
            return conversation.clone();
        }
        let mut into = Encoder::new("object");
        into.text(conversation.as_str());
        into.text(match self {
            Self::Metadata => "conversation.metadata",
            Self::Timeline => "conversation.timeline",
            Self::Agents => "conversation.agents",
        });
        ObjectId::from_bytes(into.finish())
    }
}

/// Semantic roles are independent of the item's current body or status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemRole {
    User,
    AssistantBlock,
    ToolCall,
    ToolResult,
    System,
    Image,
    TurnStarted,
    TurnEnded,
    AgentStarted,
    AgentEnded,
}

impl ItemRole {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::AssistantBlock => "assistant-block",
            Self::ToolCall => "tool-call",
            Self::ToolResult => "tool-result",
            Self::System => "system",
            Self::Image => "image",
            Self::TurnStarted => "turn-started",
            Self::TurnEnded => "turn-ended",
            Self::AgentStarted => "agent-started",
            Self::AgentEnded => "agent-ended",
        }
    }
}

/// The same native item on another branch retains this key. Body revisions
/// and branch membership are data stored beside it, not hash inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemKey {
    pub conversation: ObjectId,
    pub role: ItemRole,
    pub native: NativeItemKey,
}

impl ItemKey {
    pub fn id(&self) -> ObjectId {
        let mut into = Encoder::new("item");
        into.text(self.conversation.as_str());
        into.text(self.role.name());
        self.native.encode(&mut into);
        ObjectId::from_bytes(into.finish())
    }
}

/// A stable turn identity, distinct from its canonical display sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnKey {
    pub conversation: ObjectId,
    pub native: NativeTurnKey,
}

impl TurnKey {
    pub fn id(&self) -> ObjectId {
        let mut into = Encoder::new("turn");
        into.text(self.conversation.as_str());
        self.native.encode(&mut into);
        ObjectId::from_bytes(into.finish())
    }
}

/// A not-yet-native item observed within one source activation. The source
/// identity must survive file moves; activation must not restart its numbering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionalKey {
    pub conversation: ObjectId,
    pub role: ItemRole,
    pub source: SourceId,
    pub activation: NativeId,
    pub draft: NativeId,
}

impl ProvisionalKey {
    pub fn id(&self) -> ObjectId {
        let mut into = Encoder::new("provisional");
        into.text(self.conversation.as_str());
        into.text(self.role.name());
        into.text(self.source.as_str());
        into.text(self.activation.as_str());
        into.text(self.draft.as_str());
        ObjectId::from_bytes(into.finish())
    }
}

/// Directed relationships have stable keys; timestamps, labels and selection
/// state may change without renaming the relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationRole {
    ParentChild,
    ForkedFrom,
    ReplyTo,
    ToolResult,
    Alias,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationKey {
    pub conversation: ObjectId,
    pub role: RelationRole,
    pub from: ObjectId,
    pub to: ObjectId,
}

impl RelationKey {
    pub fn id(&self) -> ObjectId {
        let mut into = Encoder::new("relation");
        into.text(self.conversation.as_str());
        into.text(match self.role {
            RelationRole::ParentChild => "parent-child",
            RelationRole::ForkedFrom => "forked-from",
            RelationRole::ReplyTo => "reply-to",
            RelationRole::ToolResult => "tool-result",
            RelationRole::Alias => "alias",
        });
        into.text(self.from.as_str());
        into.text(self.to.as_str());
        ObjectId::from_bytes(into.finish())
    }
}
