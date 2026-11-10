//! Public facts extracted before publication. Large values are typed, owned
//! parts; neither a display summary nor private host evidence is their source.
use super::{ItemIdentity, PlacementProof};
use crate::{
    session::{AgentAct, ToolStatus},
    sync::{
        conversation_identity::{NativeId, NativeItemKey, ProvisionalKey, TurnKey},
        cursor::Count,
        identity::{HostGeneration, ObjectId, SourceId},
        manifest::ContentRef,
    },
};
use serde::{Deserialize, Serialize};

pub const FACTS_VERSION: u16 = 1;
pub const FACT_PARTS: usize = 16;

/// Fixed-size observations copied into a page. Unknown counters stay unknown;
/// message usage is an association, never an instruction to sum every item.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayFactsV1 {
    pub at: Option<jiff::Timestamp>,
    pub started_at: Option<jiff::Timestamp>,
    pub duration_ms: Option<u64>,
    pub tokens: Option<MessageTokensV1>,
    pub tool_status: Option<ToolStatus>,
    pub exit_code: Option<i64>,
    /// A shared TurnMapping part, located through the item's canonical turn.
    pub turn: Option<ContentRef>,
    /// Native coordinates, including an explicitly observed draft identity.
    pub native: Option<ContentRef>,
    /// The complete Tool or Message part, independent of the native detail.
    pub semantic: Option<ContentRef>,
    /// A typed event payload. Session-channel events are separate items; an
    /// unknown block or tool diagnostic keeps its own item's placement.
    pub event: Option<ContentRef>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageTokensV1 {
    pub input: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_creation: Option<u64>,
    pub output: Option<u64>,
    pub thinking: Option<u64>,
}

/// The payload of an ItemFactsV1 Metadata page. Each variant declares all of its
/// owned edges. Text references contain UTF-8; JSON references have the explicit
/// external-content contract stated on their field, not a private JSONL row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemFactBodyV1 {
    pub version: u16,
    pub fact: ItemFactV1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ItemFactV1 {
    Native {
        identity: ItemIdentity,
        draft: Option<ProvisionalKey>,
        source: SourceId,
        generation: HostGeneration,
        /// Physical one-based source line; absent for non-file producers.
        line: Option<Count>,
        block: Option<Count>,
        response: Option<NativeId>,
        context: Option<ContentRef>,
    },
    NativeRow {
        uuid: Option<NativeId>,
        parent: Option<NativeId>,
        logical_parent: Option<NativeId>,
        source_assistant: Option<NativeId>,
        host_kind: ContentRef,
        origin: Option<ContentRef>,
        flags: NativeRowFlagsV1,
        /// An ordered NativeId parts tree of explicitly superseded row UUIDs.
        supersedes: Option<ContentRef>,
    },
    TurnMapping {
        /// The canonical target selected by the shared producer TurnBinding.
        /// Stronger native evidence need not hash to a new canonical identity.
        turn: ObjectId,
        key: TurnKey,
        proof: PlacementProof,
    },
    Tool {
        /// Exact UTF-8 name and optional MCP namespace, without display clipping.
        name: Option<ContentRef>,
        server: Option<ContentRef>,
        machine: Option<crate::MachineName>,
        source: Option<ToolSourceV1>,
        does: Option<Box<ToolActionV1>>,
        /// The complete tool argument JSON object; absent until the call arrives.
        input: Option<ContentRef>,
        /// A typed Parts/PartIndex tree, in the tool's reported order.
        output: Option<ContentRef>,
        /// The host's structured toolUseResult JSON value, when present.
        structured: Option<ContentRef>,
        /// A typed ToolDetail retaining both native requested and result JSON.
        detail: ContentRef,
    },
    ToolDetail {
        uuid: Option<NativeId>,
        requested: Option<ContentRef>,
        requested_machine: Option<crate::MachineName>,
        stored: Option<ToolResultDetailV1>,
    },
    Message {
        response: Option<NativeId>,
        model: Option<ContentRef>,
        stop_reason: Option<ContentRef>,
        api_error: bool,
        /// The provider's complete message usage JSON, not a request total.
        usage: Option<ContentRef>,
        /// The native prose block or user-message detail JSON.
        detail: ContentRef,
    },
    Session {
        event: SessionFactV1,
    },
    /// A bounded leaf. Larger lists use a balanced typed PartIndex tree, so
    /// content supply paths grow with tree depth rather than list length.
    Parts {
        part_kind: FactPartKindV1,
        start: Count,
        parts: Vec<FactPartV1>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactPartKindV1 {
    Output,
    Change,
    NativeItem,
    Text,
    NativeId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSourceV1 {
    Native,
    Mcp,
    World,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "as", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolActionV1 {
    Command { command: ContentRef, cwd: Option<ContentRef>, job: Option<ContentRef> },
    Edit { changes: ContentRef },
    Search { query: ContentRef },
    Image { path: Option<ContentRef> },
    Colleague { to: Option<ContentRef>, task: Option<ContentRef> },
    Agent { act: AgentAct, agents: Option<ContentRef>, agent_kind: Option<ContentRef>, task: Option<ContentRef> },
    Call,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactPartV1 {
    Output { output: ToolOutputV1 },
    Change { path: ContentRef, how: FactChangeV1, diff: FactTextV1 },
    NativeItem { item: NativeItemKey },
    Text { text: ContentRef },
    NativeId { value: NativeId },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactChangeV1 {
    Added,
    Deleted,
    Updated,
    Moved { to: ContentRef },
    Replaced,
}
/// A complete string, or an ordered Parts tree of already-normalized text.
/// Segments permit a large MultiEdit diff without rebuilding earlier hunks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "text", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactTextV1 {
    Content { content: ContentRef },
    Parts { parts: ContentRef },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolOutputV1 {
    Text {
        text: ContentRef,
    },
    Image {
        mime: ContentRef,
        /// Exact UTF-8 base64 supplied by the external content contract.
        base64: ContentRef,
    },
    ImageUrl {
        url: ContentRef,
    },
    AudioUrl {
        url: ContentRef,
    },
    /// An external MCP content block, including its extension fields/annotations.
    Other {
        block: ContentRef,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResultDetailV1 {
    pub uuid: Option<NativeId>,
    pub block: ContentRef,
    pub structured: Option<ContentRef>,
    pub source_assistant: Option<NativeId>,
    /// Names in the first textual result header, applied only by World tools.
    pub machine: Option<crate::MachineName>,
    pub colleague: Option<ContentRef>,
    pub agent: Option<ContentRef>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionFactV1 {
    Title {
        custom: bool,
        title: Option<ContentRef>,
    },
    MessageUsage {
        response: Option<NativeId>,
        model: ContentRef,
        stop_reason: Option<ContentRef>,
        api_error: bool,
        usage: Option<ContentRef>,
        /// Parts of NativeItem values; this is a relation, not duplicate usage.
        items: ContentRef,
    },
    Compaction {
        boundary: bool,
        summary: bool,
        /// The boundary witnessed by this row, including an attributed summary.
        boundary_uuid: Option<NativeId>,
        /// Only a full compact boundary resets the model context.
        replaces_context: bool,
        preserved: Option<PreservedContextV1>,
        detail: ContentRef,
    },
    ContextRow {
        /// NativeRow facts; includes rows not displayed as prose or tools.
        row: ContentRef,
    },
    /// Evidence about one exchange only. This is not a native TurnEnded event.
    ExchangeEnded {
        response: Option<NativeId>,
        reason: ContentRef,
    },
    Hidden {
        host_kind: ContentRef,
        detail: ContentRef,
    },
    Other {
        host_kind: ContentRef,
        detail: ContentRef,
    },
    Diagnostic {
        message: ContentRef,
        detail: Option<ContentRef>,
    },
}

/// Native context membership flags, observed independently of display visibility.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRowFlagsV1 {
    pub meta: bool,
    pub virtual_message: bool,
    pub compact_summary: bool,
    pub transcript_only: bool,
    pub turn_companion: bool,
    pub dispatch_echo: bool,
    pub stacked_expansion: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreservedContextV1 {
    pub anchor: NativeId,
    pub selection: PreservedSelectionV1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "selection", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreservedSelectionV1 {
    /// An ordered NativeId parts tree; absence and duplicate UUIDs remain errors.
    Messages { uuids: ContentRef },
    /// The inclusive parentUuid chain, read from tail back to head.
    Segment { head: NativeId, tail: NativeId },
}
