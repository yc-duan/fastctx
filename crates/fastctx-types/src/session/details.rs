//! Presentation facts that cannot be reduced to a tool label or plain text.
//! JSON fields here are external content contracts, not formatted strings.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A user's original blocks and identity, including text spans, attachments,
/// mentions and hook provenance. `Heard.text` is the readable projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessage {
    pub id: String,
    pub client_id: Option<String>,
    pub content: Vec<Value>,
    pub hook_fragments: Vec<Value>,
    #[serde(default)]
    pub presentation: Value,
}

/// State updates do not imply a chat bubble. Opaque values preserve the
/// published contract of the corresponding host feature for richer clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionChange {
    Diff { diff: String },
    Terminal { item: String, process: String, input: String },
    Usage { usage: Value },
    Status { status: Value },
    Metadata { metadata: Value },
    Lifecycle { state: String },
    Goal { goal: Value },
    Hook { run: Value },
    ApprovalReview { review: Value, complete: bool },
    StrictReview { notice: Value },
    Safety { buffering: Value },
    Rerouted { from: String, to: String, reason: String },
    Verification { verifications: Vec<Value> },
    Error { error: Value, will_retry: bool },
    Warning { text: String },
    Settings { settings: Value },
    Environment { environment: Value, connected: bool },
    Queue,
    ToolServer { server: String, status: String, error: Value, failure_reason: Value },
    Attachment { attachment: Value },
    Request { id: Value, request: String, params: Value },
    Resolved { id: Value, resolution: Value },
    Voice { event: String, data: Value },
}
