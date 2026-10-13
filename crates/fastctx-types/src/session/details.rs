//! Presentation facts that cannot be reduced to a tool label or plain text.
//! JSON fields here are external content contracts, not formatted strings.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Health, HealthKind};

/// A user's original blocks and identity, including text spans, attachments,
/// mentions and hook provenance. `Heard.text` is the readable projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessage {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::attachments::Attachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<super::images::UserImage>,
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
                // The use and the reason OpenAI gives, such as cyber and
                // probes, say what the person who asked may want to change.
                let listed = |key: &str| {
                    let words: Vec<&str> =
                        buffering[key].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                    if words.is_empty() { "unspecified".to_string() } else { words.join(", ") }
                };
                let text = format!(
                    "OpenAI is holding this turn's output for extra safety processing (use: {}; reason: {})",
                    listed("useCases"),
                    listed("reasons")
                );
                (HealthKind::Safety, text)
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
    RunFactsUpdated {
        call_id: String,
        update: Box<crate::run_facts::JobFacts>,
    },
    /// The request parameters handed to the host; effort can be normalized by it.
    ModelRequest {
        step: u64,
        model: String,
        effort: Value,
    },
    /// The model and counts the answer actually reported. Missing usage stays unknown.
    ModelResponse {
        step: u64,
        usage: Option<super::RequestUsage>,
    },
    ModelSwitch {
        from_model: String,
        to_model: String,
        source: String,
        requested_model: Option<String>,
        raw: Value,
    },
    /// A stored assistant row's model and usage, distinct from a request total.
    MessageUsage {
        items: Vec<String>,
        response: Option<String>,
        model: String,
        api_error: bool,
        usage: Value,
    },
    /// Command descriptions changed; refresh the session's offered catalog.
    CommandsChanged,
    /// The host's own agents as it lists them now, with how each stands.
    Agents {
        agents: Vec<super::HostAgent>,
    },
    /// One of the host's own agents started, or more is known of it.
    AgentStarted {
        agent: Box<super::HostAgentStart>,
    },
    /// What one of the host's own agents did, in its own loop: one event of
    /// its own conversation.
    AgentEvent {
        agent_id: String,
        happened: Box<super::Happened>,
    },
    /// One of the host's own agents ended its latest run.
    AgentEnded {
        agent_id: String,
        end: super::HostAgentEnd,
    },
    /// Current input state, outside the host transcript and replaced as a whole.
    Composer {
        composer: Box<crate::composer::Composer>,
    },
    /// A shared-input checkpoint failed; observers receive the actual failure.
    ComposerFailed {
        error: String,
    },
    /// Re-read host history: a stable host operation removed or replaced
    /// turns. This says nothing about restoring files on disk.
    HistoryChanged,
    HistoryCut {
        removed_turns: Vec<String>,
    },
    /// The World created this resident from another thread's history.
    Forked {
        source: crate::Address,
        thread: String,
        through: Option<String>,
    },
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
        /// Opaque reply address, passed unchanged to Control::Answer.
        /// Codex wraps its host ID with the app-server instance identity.
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
