//! Index only fields used by Claude semantics. Other values retain their exact
//! raw span and shape, without indexing or duplicating their descendants.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Selection {
    Row,
    Message,
    Blocks,
    Block,
    Source,
    Input,
    Edits,
    Edit,
    Origin,
    Data,
    Compact,
    Preserved,
    Segment,
    Strings,
    Attachment,
    AgentResult,
    Scalar,
    Opaque,
}
impl Selection {
    pub fn container(self, object: bool) -> Self {
        match (self, object) {
            (Self::Blocks | Self::Strings | Self::Edits, false) => self,
            (Self::Opaque | Self::Scalar | Self::Blocks | Self::Strings | Self::Edits, _) | (_, false) => Self::Opaque,
            _ => self,
        }
    }
    pub fn child(self, key: Option<&str>) -> Self {
        use Selection::*;
        match (self, key) {
            (Blocks, _) => Block,
            (Strings, _) => Scalar,
            (Edits, _) => Edit,
            (Row, Some("message")) => Message,
            (Row, Some("origin")) => Origin,
            (Row, Some("data")) => Data,
            (Row, Some("compactMetadata")) => Compact,
            (Row, Some("attachment")) => Attachment,
            (Row, Some("toolUseResult")) => AgentResult,
            (Row, Some("supersedesUuids" | "retractedMessageUuids")) => Strings,
            (Message | Block | AgentResult, Some("content")) => Blocks,
            (Block, Some("source")) => Source,
            (Block, Some("input")) => Input,
            (Input, Some("edits")) => Edits,
            (Compact, Some("preservedMessages")) => Preserved,
            (Compact, Some("preservedSegment")) => Segment,
            (Preserved, Some("uuids")) => Strings,
            (
                Row,
                Some(
                    "type"
                    | "uuid"
                    | "timestamp"
                    | "isSidechain"
                    | "agentId"
                    | "parentUuid"
                    | "logicalParentUuid"
                    | "sourceToolAssistantUUID"
                    | "sourceToolUseID"
                    | "isMeta"
                    | "isCompactSummary"
                    | "isVisibleInTranscriptOnly"
                    | "isVirtual"
                    | "turnCompanion"
                    | "dispatchEcho"
                    | "stackedExpansion"
                    | "promptSource"
                    | "turnOrigin"
                    | "subtype"
                    | "isApiErrorMessage"
                    | "thinkingDurationMs"
                    | "customTitle"
                    | "aiTitle"
                    | "toolUseId"
                    | "agentType"
                    | "description",
                ),
            ) => Scalar,
            (Message, Some("role" | "id" | "model")) => Scalar,
            (Block, Some("type" | "text" | "thinking" | "data" | "id" | "name" | "tool_use_id" | "is_error")) => Scalar,
            (Source, Some("type" | "media_type" | "data")) => Scalar,
            (
                Input,
                Some(
                    "command" | "file_path" | "old_string" | "new_string" | "content" | "query" | "subagent_type"
                    | "description" | "name" | "prompt" | "run_in_background" | "to" | "message" | "text" | "node"
                    | "cwd" | "input",
                ),
            ) => Scalar,
            (Edit, Some("old_string" | "new_string")) => Scalar,
            (Origin, Some("kind")) | (Data, Some("type")) => Scalar,
            (Preserved, Some("anchorUuid")) | (Segment, Some("anchorUuid" | "headUuid" | "tailUuid")) => Scalar,
            (Attachment, Some("type" | "source_uuid")) => Scalar,
            (
                AgentResult,
                Some(
                    "agentId" | "agentType" | "prompt" | "resolvedModel" | "isAsync" | "status" | "totalDurationMs"
                    | "totalTokens" | "totalToolUseCount",
                ),
            ) => Scalar,
            _ => Opaque,
        }
    }
}
