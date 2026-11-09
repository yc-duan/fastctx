//! Original host coordinates and explicitly weaker legacy observations.

use super::{IdentityError, encoding::Encoder};
use crate::{NoticeId, sync::cursor::Count};
use serde::{Deserialize, Serialize};

/// An opaque, nonempty native field. Case, whitespace and Unicode are preserved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NativeId(String);

impl NativeId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityError> {
        Self::try_from(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NativeId {
    type Error = IdentityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() { Err(IdentityError::EmptyNativeId) } else { Ok(Self(value)) }
    }
}

impl From<NativeId> for String {
    fn from(value: NativeId) -> Self {
        value.0
    }
}

/// The exact ten-byte digest surviving in old World event identifiers.
/// It is not expanded into, or asserted to identify, a lost NoticeId.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LegacyDigest(String);

impl LegacyDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LegacyDigest {
    type Error = IdentityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 20 || !value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
            return Err(IdentityError::InvalidLegacyDigest);
        }
        Ok(Self(value))
    }
}

impl From<LegacyDigest> for String {
    fn from(value: LegacyDigest) -> Self {
        value.0
    }
}

/// A child host's own identifier, relative to its canonical parent conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "host", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeAgentKey {
    Claude { agent: NativeId },
    Codex { thread: NativeId },
}

impl NativeAgentKey {
    pub(super) fn encode(&self, into: &mut Encoder) {
        match self {
            Self::Claude { agent } => {
                into.text("claude-agent");
                into.text(agent.as_str());
            }
            Self::Codex { thread } => {
                into.text("codex-agent");
                into.text(thread.as_str());
            }
        }
    }
}

/// A source-provided turn name, not its display position or arrival timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "host", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeTurnKey {
    /// Plugin turn names are scoped to their original host session.
    Claude {
        session: NativeId,
        turn: NativeId,
    },
    Codex {
        thread: NativeId,
        turn: NativeId,
    },
    World {
        turn: NativeId,
    },
}

impl NativeTurnKey {
    pub(super) fn encode(&self, into: &mut Encoder) {
        match self {
            Self::Claude { session, turn } => {
                into.text("claude-turn");
                into.text(session.as_str());
                into.text(turn.as_str());
            }
            Self::Codex { thread, turn } => {
                into.text("codex-turn");
                into.text(thread.as_str());
                into.text(turn.as_str());
            }
            Self::World { turn } => {
                into.text("world-turn");
                into.text(turn.as_str());
            }
        }
    }
}

/// A picture's logical position, independent of its bytes or current filename.
/// Image indexes count images, not all content blocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImageSelector {
    ClaudeMessage {
        uuid: NativeId,
        image: Count,
    },
    ClaudeTool {
        tool_use_id: NativeId,
        image: Count,
    },
    CodexItem {
        thread: NativeId,
        turn: NativeId,
        item: NativeId,
        image: Count,
    },
    /// A legacy ordinal is an observation until a receipt identifies its item.
    LegacyCodexMessage {
        domain: NativeId,
        turn: NativeId,
        message: Count,
        image: Count,
    },
}

impl ImageSelector {
    pub(super) fn encode(&self, into: &mut Encoder) {
        match self {
            Self::ClaudeMessage { uuid, image } => {
                into.text("claude-message-image");
                into.text(uuid.as_str());
                into.index(image.0);
            }
            Self::ClaudeTool { tool_use_id, image } => {
                into.text("claude-tool-image");
                into.text(tool_use_id.as_str());
                into.index(image.0);
            }
            Self::CodexItem { thread, turn, item, image } => {
                into.text("codex-item-image");
                into.text(thread.as_str());
                into.text(turn.as_str());
                into.text(item.as_str());
                into.index(image.0);
            }
            Self::LegacyCodexMessage { domain, turn, message, image } => {
                into.text("legacy-codex-message-image");
                into.text(domain.as_str());
                into.text(turn.as_str());
                into.index(message.0);
                into.index(image.0);
            }
        }
    }
}

/// Original item coordinates. A continued Claude file may repeat a UUID;
/// its new session, physical path and branch are deliberately not fields here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeItemKey {
    ClaudeMessage {
        uuid: NativeId,
    },
    ClaudeBlock {
        uuid: NativeId,
        block: Count,
    },
    ClaudeTool {
        tool_use_id: NativeId,
    },
    CodexItem {
        thread: NativeId,
        turn: NativeId,
        item: NativeId,
    },
    WorldNotice {
        notice: NoticeId,
    },
    WorldCall {
        call: NativeId,
    },
    Turn {
        turn: NativeTurnKey,
    },
    Agent {
        agent: NativeAgentKey,
    },
    Image {
        selector: ImageSelector,
    },
    /// Domain identifies the immutable original source/family, not its path.
    LegacyEvent {
        domain: NativeId,
        digest: LegacyDigest,
    },
    /// An original opaque World key without a proven native alias.
    LegacyRecord {
        domain: NativeId,
        key: NativeId,
    },
}

impl NativeItemKey {
    pub fn is_legacy(&self) -> bool {
        matches!(
            self,
            Self::LegacyEvent { .. }
                | Self::LegacyRecord { .. }
                | Self::Image { selector: ImageSelector::LegacyCodexMessage { .. } }
        )
    }

    pub(super) fn encode(&self, into: &mut Encoder) {
        match self {
            Self::ClaudeMessage { uuid } => {
                into.text("claude-message");
                into.text(uuid.as_str());
            }
            Self::ClaudeBlock { uuid, block } => {
                into.text("claude-block");
                into.text(uuid.as_str());
                into.index(block.0);
            }
            Self::ClaudeTool { tool_use_id } => {
                into.text("claude-tool");
                into.text(tool_use_id.as_str());
            }
            Self::CodexItem { thread, turn, item } => {
                into.text("codex-item");
                into.text(thread.as_str());
                into.text(turn.as_str());
                into.text(item.as_str());
            }
            Self::WorldNotice { notice } => {
                into.text("world-notice");
                into.text(notice.as_str());
            }
            Self::WorldCall { call } => {
                into.text("world-call");
                into.text(call.as_str());
            }
            Self::Turn { turn } => {
                into.text("turn");
                turn.encode(into);
            }
            Self::Agent { agent } => {
                into.text("agent");
                agent.encode(into);
            }
            Self::Image { selector } => {
                into.text("image");
                selector.encode(into);
            }
            Self::LegacyEvent { domain, digest } => {
                into.text("legacy-event");
                into.text(domain.as_str());
                into.text(digest.as_str());
            }
            Self::LegacyRecord { domain, key } => {
                into.text("legacy-record");
                into.text(domain.as_str());
                into.text(key.as_str());
            }
        }
    }
}
