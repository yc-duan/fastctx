use super::fields;
use crate::claude::ingest::document::{Address, Value, read::Reader};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Header {
    pub kind: String,
    pub problem: Option<String>,
    pub uuid: Option<String>,
    pub at: Option<Timestamp>,
    pub message: Option<Address>,
    pub content: Option<Address>,
    pub response: Option<String>,
    pub parent: Option<String>,
    pub source_assistant: Option<String>,
    pub agent: Option<String>,
    pub origin: Option<String>,
    pub prompt_candidate: bool,
    pub compact_summary: bool,
    pub compact_boundary: bool,
    pub source_assistant_invalid: bool,
    pub supersedes: Option<Address>,
    pub queued: Option<String>,
}
#[derive(Clone, Debug)]
pub enum RowKind {
    Skipped,
    Invalid { reason: String, queued: Option<String> },
    Unindexed,
    Title { custom: bool, text: Option<crate::claude::ingest::document::Span>, at: Option<Timestamp> },
    Row(Box<Header>),
}
/// Filtering precedes UUID replacement, matching the legacy transcript reader.
/// Invalid message bodies still have a row identity and may revise an earlier row.
pub async fn header(reader: &Reader<'_>, agent: Option<&str>) -> io::Result<RowKind> {
    let root = reader.document.root;
    if matches!(
        reader.node(root).await?.value,
        Value::Opaque { reason: crate::claude::ingest::document::Unindexed::RowBudget, .. }
    ) {
        return Ok(RowKind::Unindexed);
    }
    let Some(kind) = fields::summary(reader, root, "type", 256).await? else {
        return Ok(RowKind::Invalid { reason: "expected an object with a string type".into(), queued: None });
    };
    let at = match super::message::string(reader, root, "timestamp").await? {
        Some(span) => reader.text(span, 128).await?.and_then(|text| text.parse().ok()),
        None => None,
    };
    if matches!(kind.as_str(), "custom-title" | "ai-title") {
        let custom = kind == "custom-title";
        return Ok(RowKind::Title {
            custom,
            text: super::message::string(reader, root, if custom { "customTitle" } else { "aiTitle" }).await?,
            at,
        });
    }
    let sidechain = match fields::boolean(reader, root, "isSidechain").await {
        Ok(value) => value.unwrap_or(false),
        Err(error) if fields::is_field_error(&error) => {
            return Ok(RowKind::Invalid { reason: error.to_string(), queued: None });
        }
        Err(error) => return Err(error),
    };
    let native_agent = fields::text(reader, root, "agentId").await?;
    if fields::present(reader, root, "agentId").await? && native_agent.is_none() {
        return Ok(RowKind::Invalid { reason: "agentId is not a string or null".into(), queued: None });
    }
    if match agent {
        Some(agent) => native_agent.as_deref() != Some(agent),
        None => sidechain || native_agent.is_some(),
    } {
        return Ok(RowKind::Skipped);
    }
    if kind == "progress"
        && let Some(data) = fields::object(reader, root, "data").await?
        && matches!(fields::text(reader, data, "type").await?.as_deref(), Some("agent_progress" | "skill_progress"))
    {
        return Ok(RowKind::Skipped);
    }
    if bookkeeping(&kind) {
        return Ok(RowKind::Skipped);
    }
    let queued = if kind == "attachment" {
        if let Some(attachment) = fields::object(reader, root, "attachment").await? {
            if fields::text(reader, attachment, "type").await?.as_deref() == Some("queued_command") {
                fields::text(reader, attachment, "source_uuid").await?.filter(|s| !s.is_empty())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    let uuid = fields::text(reader, root, "uuid").await?;
    if reader.field(root, "uuid").await?.is_some() && uuid.as_ref().is_none_or(String::is_empty) {
        return Ok(RowKind::Invalid { reason: "uuid is not a nonempty string".into(), queued });
    }

    let message = fields::object(reader, root, "message").await?;
    let content =
        if let Some(message) = message { reader.field(message, "content").await?.map(|(a, _)| a) } else { None };
    let response = if let Some(message) = message { fields::text(reader, message, "id").await? } else { None };
    let origin = if let Some(origin) = fields::object(reader, root, "origin").await? {
        fields::summary(reader, origin, "kind", 256).await?
    } else {
        None
    };
    let is_message = matches!(kind.as_str(), "user" | "assistant");
    let mut problem = None;
    if is_message {
        if uuid.is_none() {
            problem = Some("message has no uuid".into());
        }
        if message.is_none() {
            problem = Some("missing or invalid message".into());
        }
        if let Some(message) = message
            && reader.field(message, "role").await?.is_some()
            && fields::text(reader, message, "role").await?.as_deref() != Some(kind.as_str())
        {
            problem = Some("invalid message role".into());
        }
        let shape = match content {
            Some(content) => Some(reader.node(content).await?.value),
            None => None,
        };
        if !(matches!(shape, Some(Value::Array { .. }))
            || kind == "user" && matches!(shape, Some(Value::String { .. })))
        {
            problem = Some("message content is not a supported string or block array".into());
        }
        if fields::present(reader, root, "origin").await? && origin.is_none() {
            problem = Some("invalid message origin".into());
        }
    }
    let mut prompt_candidate = kind == "user";
    let mut compact_summary = false;
    for flag in [
        "isCompactSummary",
        "isVisibleInTranscriptOnly",
        "isVirtual",
        "turnCompanion",
        "dispatchEcho",
        "stackedExpansion",
    ] {
        if is_message && message_flag(reader, root, flag, &mut problem).await? {
            prompt_candidate = false;
            compact_summary |= flag == "isCompactSummary";
        }
    }
    for flag in ["sourceToolAssistantUUID", "sourceToolUseID", "toolUseResult"] {
        if fields::present(reader, root, flag).await? {
            prompt_candidate = false;
        }
    }
    if is_message
        && message_flag(reader, root, "isMeta", &mut problem).await?
        && fields::text(reader, root, "promptSource").await?.is_none()
        && fields::text(reader, root, "turnOrigin").await?.is_none()
    {
        prompt_candidate = false;
    }
    let subtype = fields::text(reader, root, "subtype").await?;
    let source_assistant = fields::text(reader, root, "sourceToolAssistantUUID").await?;
    let source_assistant_invalid =
        fields::present(reader, root, "sourceToolAssistantUUID").await? && source_assistant.is_none();
    let supersedes = if kind == "assistant" {
        Some("supersedesUuids")
    } else if kind == "system" && subtype.as_deref() == Some("model_refusal_fallback") {
        Some("retractedMessageUuids")
    } else {
        None
    };
    let supersedes = match supersedes {
        Some(name) => reader.field(root, name).await?.map(|(address, _)| address),
        None => None,
    };
    Ok(RowKind::Row(Box::new(Header {
        kind: kind.clone(),
        problem,
        uuid,
        at,
        message,
        content,
        response,
        parent: fields::text(reader, root, "parentUuid").await?,
        source_assistant,
        source_assistant_invalid,
        supersedes,
        queued,
        agent: native_agent,
        origin,
        prompt_candidate,
        compact_summary,
        compact_boundary: kind == "system"
            && matches!(subtype.as_deref(), Some("compact_boundary" | "microcompact_boundary")),
    })))
}
async fn message_flag(
    reader: &Reader<'_>,
    root: Address,
    name: &str,
    problem: &mut Option<String>,
) -> io::Result<bool> {
    match fields::boolean(reader, root, name).await {
        Ok(value) => Ok(value.unwrap_or(false)),
        Err(error) if fields::is_field_error(&error) => {
            *problem = Some(error.to_string());
            Ok(false)
        }
        Err(error) => Err(error),
    }
}
fn bookkeeping(kind: &str) -> bool {
    matches!(
        kind,
        "summary"
            | "custom-title"
            | "ai-title"
            | "last-prompt"
            | "tag"
            | "agent-name"
            | "agent-color"
            | "agent-setting"
            | "mode"
            | "permission-mode"
            | "atis-latch"
            | "isolation-latch"
            | "dev-mods"
            | "worktree-state"
            | "cost-state"
            | "tools-used"
            | "file-history-snapshot"
            | "file-history-delta"
            | "attribution-snapshot"
            | "artifact-comment-monitor"
            | "artifact-autoreact-ledger"
            | "bridge-session"
            | "history-suppression"
            | "api-request-shape"
            | "api-request-blob"
            | "api-request"
            | "observer-ref"
            | "fork-context-ref"
            | "relocated"
    )
}
