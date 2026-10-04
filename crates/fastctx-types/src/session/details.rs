//! Presentation facts that cannot be reduced to a tool label or plain text.
//! JSON fields here are external content contracts, not formatted strings.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Health, HealthKind};

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
    #[serde(default)]
    pub detail: Value,
}

impl SessionChange {
    /// What the change says of the session's own health, if anything: it is
    /// then told as any health report is (`world-colleagues.md`, 一轮怎样
    /// 结束，会话健不健康), by the node to whoever is owed the turn, and by
    /// every client that shows the session.
    pub fn health(&self) -> Option<Health> {
        let (kind, text) = match self {
            SessionChange::Rerouted { from, to, reason } => {
                (HealthKind::Rerouted, format!("OpenAI moved this turn from {from} to {to} ({reason})"))
            }
            // Codex shows its own notice only when the host asks for it.
            SessionChange::Safety { buffering } if buffering["showBufferingUi"] == true => {
                (HealthKind::Safety, "OpenAI is holding this turn's output for extra safety processing".into())
            }
            SessionChange::Error { error, will_retry } => (
                if *will_retry { HealthKind::Reconnecting } else { HealthKind::Error },
                error.get("message")?.as_str()?.to_string(),
            ),
            SessionChange::ToolServer { server, status, error, .. } if status == "failed" => (
                HealthKind::ToolServer,
                format!("its {server} tools did not start: {}", error.as_str().unwrap_or("no reason given")),
            ),
            SessionChange::Warning { text } => (HealthKind::Warning, text.clone()),
            _ => return None,
        };
        Some(Health { kind, text })
    }
}

/// State updates do not imply a chat bubble. Opaque values preserve the
/// published contract of the corresponding host feature for richer clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionChange {
    Diff {
        diff: String,
    },
    Terminal {
        item: String,
        process: String,
        input: String,
    },
    Usage {
        usage: Value,
    },
    Status {
        status: Value,
        /// Whether the host is waiting for a human answer; absent when its
        /// status is not understood by the host adapter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        waiting_for_user: Option<bool>,
    },
    Metadata {
        metadata: Value,
    },
    Lifecycle {
        state: String,
    },
    Goal {
        goal: Value,
    },
    Hook {
        run: Value,
    },
    ApprovalReview {
        review: Value,
        complete: bool,
    },
    StrictReview {
        notice: Value,
    },
    Safety {
        buffering: Value,
    },
    Rerouted {
        from: String,
        to: String,
        reason: String,
    },
    Verification {
        verifications: Vec<Value>,
    },
    Error {
        error: Value,
        will_retry: bool,
    },
    Warning {
        text: String,
    },
    Settings {
        settings: Value,
    },
    Environment {
        environment: Value,
        connected: bool,
    },
    Queue,
    ToolServer {
        server: String,
        status: String,
        error: Value,
        #[serde(default)]
        failure_reason: Value,
    },
    Attachment {
        attachment: Value,
    },
    Request {
        id: Value,
        request: String,
        params: Value,
    },
    Resolved {
        id: Value,
        resolution: Value,
    },
    Voice {
        event: String,
        data: Value,
    },
}
