//! Extract native identity fields without reading files, visiting message bodies,
//! assigning ownership, or choosing canonical identities. Callers pass the
//! enclosing thread/session and indexes from the host's own ordering.

use fastctx_types::{
    attachments::{HostImage, ToolImage},
    sync::{
        conversation_identity::{ImageSelector, NativeAgentKey, NativeId, NativeItemKey, NativeTurnKey},
        cursor::Count,
    },
};
use serde_json::Value;

/// Missing identity is distinguishable from malformed metadata. Neither case
/// falls back to text, a timestamp, an array position, or a filesystem path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Missing(&'static str),
    Invalid(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(field) => write!(out, "missing native identity field: {field}"),
            Self::Invalid(field) => write!(out, "invalid native identity field: {field}"),
        }
    }
}

impl std::error::Error for Error {}

fn id(value: &str, field: &'static str) -> Result<NativeId, Error> {
    NativeId::new(value).map_err(|_| Error::Invalid(field))
}

fn field(value: &Value, name: &'static str) -> Result<NativeId, Error> {
    match value.get(name) {
        None | Some(Value::Null) => Err(Error::Missing(name)),
        Some(Value::String(text)) => id(text, name),
        Some(_) => Err(Error::Invalid(name)),
    }
}

/// The UUID in a Claude transcript row or session.append result.
pub fn claude_message(record: &Value) -> Result<NativeItemKey, Error> {
    Ok(NativeItemKey::ClaudeMessage { uuid: field(record, "uuid")? })
}

/// Block indexes count all blocks, unlike image indexes. The session filename
/// and parentUuid are intentionally not part of this coordinate.
pub fn claude_block(record: &Value, block: Count) -> Result<NativeItemKey, Error> {
    Ok(NativeItemKey::ClaudeBlock { uuid: field(record, "uuid")?, block })
}

/// The existing World projection spells a stored Claude block as [uuid,index].
/// The old three-element transcript-line fallback has no native UUID and fails.
pub fn claude_stored_block(stored: &str) -> Result<NativeItemKey, Error> {
    let value: Value = serde_json::from_str(stored).map_err(|_| Error::Invalid("stored block id"))?;
    let parts = value.as_array().ok_or(Error::Invalid("stored block id"))?;
    if parts.len() == 3 && parts[0] == "transcript-line" {
        return Err(Error::Missing("uuid"));
    }
    if parts.len() != 2 {
        return Err(Error::Invalid("stored block id"));
    }
    let uuid = id(parts[0].as_str().ok_or(Error::Invalid("uuid"))?, "uuid")?;
    let block = Count(parts[1].as_u64().ok_or(Error::Invalid("block index"))?);
    Ok(NativeItemKey::ClaudeBlock { uuid, block })
}

/// Calls and results name the same native tool-use coordinate. The caller
/// supplies their distinct semantic roles to the shared ItemKey.
pub fn claude_tool(block: &Value) -> Result<NativeItemKey, Error> {
    let tool_use_id = match block.get("type").and_then(Value::as_str) {
        Some("tool_use") => field(block, "id")?,
        Some("tool_result") => field(block, "tool_use_id")?,
        _ => return Err(Error::Invalid("tool block type")),
    };
    Ok(NativeItemKey::ClaudeTool { tool_use_id })
}

/// Extract a tool-call coordinate already carried by a World item or receipt.
pub fn claude_tool_id(tool_use_id: &str) -> Result<NativeItemKey, Error> {
    Ok(NativeItemKey::ClaudeTool { tool_use_id: id(tool_use_id, "tool_use_id")? })
}

pub fn claude_turn(session: &str, turn: &str) -> Result<NativeTurnKey, Error> {
    Ok(NativeTurnKey::Claude { session: id(session, "session")?, turn: id(turn, "turn")? })
}

/// A plugin/tool result reports agentId. File discovery may instead pass the
/// already-validated filename component to claude_agent_id; neither does IO.
pub fn claude_agent(record: &Value) -> Result<NativeAgentKey, Error> {
    Ok(NativeAgentKey::Claude { agent: field(record, "agentId")? })
}

pub fn claude_agent_id(agent: &str) -> Result<NativeAgentKey, Error> {
    Ok(NativeAgentKey::Claude { agent: id(agent, "agentId")? })
}

/// Pass the thread object, not the enclosing thread/read response.
pub fn codex_thread(thread: &Value) -> Result<NativeId, Error> {
    field(thread, "id")
}

pub fn codex_turn(thread: &str, turn: &Value) -> Result<NativeTurnKey, Error> {
    Ok(NativeTurnKey::Codex { thread: id(thread, "thread")?, turn: field(turn, "id")? })
}

pub fn codex_item(thread: &str, turn: &str, item: &Value) -> Result<NativeItemKey, Error> {
    Ok(NativeItemKey::CodexItem { thread: id(thread, "thread")?, turn: id(turn, "turn")?, item: field(item, "id")? })
}

pub fn codex_agent(thread: &str) -> Result<NativeAgentKey, Error> {
    Ok(NativeAgentKey::Codex { thread: id(thread, "thread")? })
}

pub fn claude_image(record: &Value, image: Count) -> Result<ImageSelector, Error> {
    Ok(ImageSelector::ClaudeMessage { uuid: field(record, "uuid")?, image })
}

pub fn claude_tool_image(tool_use_id: &str, image: Count) -> Result<ImageSelector, Error> {
    Ok(ImageSelector::ClaudeTool { tool_use_id: id(tool_use_id, "tool_use_id")?, image })
}

pub fn codex_image(thread: &str, turn: &str, item: &Value, image: Count) -> Result<ImageSelector, Error> {
    Ok(ImageSelector::CodexItem {
        thread: id(thread, "thread")?,
        turn: id(turn, "turn")?,
        item: field(item, "id")?,
        image,
    })
}

/// Preserve the old selector without asserting that an ordinal is an item ID.
/// The caller supplies its immutable original observation domain, not a path.
pub fn legacy_host_image(source: &HostImage, domain: &str) -> Result<ImageSelector, Error> {
    match source {
        HostImage::Claude { message, image } => {
            Ok(ImageSelector::ClaudeMessage { uuid: id(message, "uuid")?, image: Count(u64::from(*image)) })
        }
        HostImage::Codex { turn, message, image } => Ok(ImageSelector::LegacyCodexMessage {
            domain: id(domain, "legacy domain")?,
            turn: id(turn, "turn")?,
            message: Count(u64::from(*message)),
            image: Count(u64::from(*image)),
        }),
    }
}

/// Codex tool-image receipts need their enclosing native thread and turn.
/// A missing turn is unresolved; it is never replaced with a current turn.
pub fn codex_tool_image(thread: &str, source: &ToolImage) -> Result<ImageSelector, Error> {
    Ok(ImageSelector::CodexItem {
        thread: id(thread, "thread")?,
        turn: id(source.turn.as_deref().ok_or(Error::Missing("turn"))?, "turn")?,
        item: id(&source.item, "item")?,
        image: Count(u64::from(source.image)),
    })
}
