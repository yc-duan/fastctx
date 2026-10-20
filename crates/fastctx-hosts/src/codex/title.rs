//! A title for a new Codex thread, made the way the official client makes
//! one (`world-agent-logins.md`, 在哪个登录里干活): once a conversation has
//! its first words, a hidden thread that persists nothing and can touch
//! nothing asks a cheap model for a short title. Nothing of the hidden thread
//! is shown; only its token deltas reach the root conversation's ledger. Its
//! answer comes back on its own channel (`State::title_waiters`). The caller asks only when the
//! login wants it and the thread has no name yet, and names the thread with
//! the title only if it still has none by then: the user may have named it
//! while the title was being made.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::items::{self, Read};
use super::{CodexHost, What};
use fastctx_types::session::{Happened, SessionChange};

pub(super) struct Usage {
    root: String,
    model: Option<String>,
    accounting: super::accounting::Accounting,
    requests: super::requests::Requests,
}
impl Usage {
    pub(super) fn new(root: String, model: Option<String>) -> Self {
        let mut accounting = super::accounting::Accounting::default();
        accounting.empty_thread();
        Self { root, model, accounting, requests: Default::default() }
    }
}

/// The cheap model the official client titles with, when the login can use
/// it; otherwise the thread's current model names itself.
const TITLE_MODEL: &str = "gpt-5.6-luna";

/// The title is at most this many characters, as the official client caps
/// it; counted in characters, not bytes.
const MAX_CHARS: usize = 36;

/// The prompt is kept under this, so a long first message does not crowd
/// out the instructions.
const PROMPT_MAX_BYTES: usize = 960;

/// How long a title may take before it is given up on.
const WITHIN: std::time::Duration = std::time::Duration::from_secs(30);

/// The instructions the official client gives, verbatim.
const INSTRUCTIONS: &str = "Generate a concise, single-line task title of at most 36 characters and under five words where possible. Start with an imperative verb. Capitalize only the first word unless the user's language, proper nouns, acronyms, or code terms require otherwise. Preserve ticket references exactly. Write in the user's language. Do not use quotes, markdown, or trailing punctuation. Do not answer the request.";

impl CodexHost {
    pub(super) fn title_usage(&self, thread: &str, method: &str, params: &Value, at: jiff::Timestamp) {
        let counted = {
            let mut state = self.lock();
            let Some(usage) = state.title_usage.get_mut(thread) else { return };
            if method == "model/rerouted" {
                usage.model = params.get("toModel").and_then(Value::as_str).map(str::to_string);
            }
            let failed = usage.requests.observe(method, params, at);
            let tokens = (method == "thread/tokenUsage/updated")
                .then(|| usage.accounting.usage(None, &params["tokenUsage"]))
                .flatten();
            let request = if tokens.is_some() {
                Some(usage.requests.finish(at, fastctx_types::usage_calls::CallOutcome::Completed))
            } else {
                failed
            };
            request.map(|request| {
                (
                    usage.root.clone(),
                    json!({ "counted": {
                        "thread": thread, "turn": params.get("turnId").or_else(|| params.pointer("/turn/id")),
                        "model": usage.model, "tokens": tokens, "request": request, "background": true,
                    }}),
                )
            })
        };
        if let Some((root, usage)) = counted {
            self.emit(&root, at, Happened::Changed { turn: None, change: SessionChange::Usage { usage } });
        }
    }

    /// Makes a title for `thread` from `user_text`; `None` when the model
    /// gave none of the shape asked for. The thread is not named here.
    pub async fn make_title(self: &Arc<Self>, thread: &str, user_text: &str) -> Result<Option<String>, String> {
        let user_text = user_text.trim();
        if user_text.is_empty() {
            return Ok(None);
        }
        let server = self.server().await?;
        let (model, cwd) = {
            let mut state = self.lock();
            let (cwd, current) = state
                .threads
                .get(thread)
                .map(|entry| (entry.spec.as_ref().map(|spec| spec.cwd.clone()), entry.model.clone()))
                .unwrap_or_default();
            let lists_luna =
                state.catalog.as_ref().is_some_and(|models| models.iter().any(|offer| offer.id == TITLE_MODEL));
            (if lists_luna { Some(TITLE_MODEL.to_string()) } else { current }, cwd)
        };
        let cwd = cwd.unwrap_or_else(|| self.workdir.clone());
        // A hidden thread that keeps nothing on disk, is listed nowhere, and
        // can neither run tools nor touch the machine.
        let started = server
            .request(
                "thread/start",
                json!({
                    "cwd": cwd,
                    "model": model,
                    "sandbox": "read-only",
                    "ephemeral": true,
                    "threadSource": "thread_title",
                    "environments": [],
                    "config": config(),
                }),
            )
            .await?;
        let temp = started
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or("the hidden title thread started without an id")?
            .to_string();
        self.lock().title_usage.insert(temp.clone(), Usage::new(thread.to_string(), model.clone()));
        let answer = self.ask_title(&server, &temp, user_text, model.as_deref() == Some(TITLE_MODEL)).await;
        self.lock().title_usage.remove(&temp);
        // The hidden thread is let go however the turn went: with no
        // subscriber and no running turn, the app-server unloads it.
        let _ = server.request("thread/unsubscribe", json!({ "threadId": temp })).await;
        Ok(clean(&answer?))
    }

    /// Runs the one titling turn and waits for its end on a channel of its
    /// own, so the hidden thread's notifications never reach the node.
    async fn ask_title(
        &self,
        server: &super::server::AppServer,
        temp: &str,
        user_text: &str,
        luna: bool,
    ) -> Result<String, String> {
        let (sender, answered) = oneshot::channel();
        self.lock().title_waiters.insert(temp.to_string(), sender);
        let result = async {
            let mut prompt = format!("{INSTRUCTIONS}\n\nUser prompt:\n{user_text}");
            truncate(&mut prompt, PROMPT_MAX_BYTES);
            let mut params = json!({
                "threadId": temp,
                "input": [{ "type": "text", "text": prompt, "textElements": [] }],
                "outputSchema": output_schema(),
            });
            if luna {
                params["effort"] = json!("low");
            }
            server.request("turn/start", params).await?;
            match tokio::time::timeout(WITHIN, answered).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err("the title turn's channel closed".to_string()),
                Err(_) => Err("the title turn did not finish in time".to_string()),
            }
        }
        .await;
        self.lock().title_waiters.remove(temp);
        result
    }
}

/// The last message of a completed title turn, from the turn's own items.
pub(super) fn read_turn_message(params: &Value) -> Option<String> {
    if params.pointer("/turn/status").and_then(Value::as_str) != Some("completed") {
        return None;
    }
    params.pointer("/turn/items").and_then(Value::as_array)?.iter().rev().find_map(|item| {
        match items::read(item, true) {
            Read::Item(entry) => match entry.what {
                What::Message { text } => Some(text),
                _ => None,
            },
            _ => None,
        }
    })
}

/// The settings the hidden thread starts with: everything that would make it
/// do more than answer with text turned off.
fn config() -> Value {
    let mut config = serde_json::Map::new();
    for feature in [
        "apps",
        "code_mode",
        "code_mode_only",
        "context_management",
        "current_time_reminder",
        "deferred_executor",
        "enable_fanout",
        "goals",
        "hooks",
        "image_generation",
        "memories",
        "multi_agent",
        "multi_agent_v2",
        "plugins",
        "request_permissions_tool",
        "shell_snapshot",
        "shell_tool",
        "standalone_web_search",
        "token_budget",
        "tool_suggest",
        "unified_exec",
        "view_image",
    ] {
        config.insert(format!("features.{feature}"), json!(false));
    }
    config.insert("cloud.skills.enabled".into(), json!(false));
    config.insert("skills.include_instructions".into(), json!(false));
    config.insert("tools.experimental_request_user_input.enabled".into(), json!(false));
    config.insert("tools.update_plan.enabled".into(), json!(false));
    config.insert("web_search".into(), json!("disabled"));
    config.insert("default_permissions".into(), json!(":read-only"));
    Value::Object(config)
}

/// The shape the model must answer in: one short title.
fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "title": { "type": "string", "minLength": 1, "maxLength": MAX_CHARS } },
        "required": ["title"],
        "additionalProperties": false,
    })
}

/// A title from the model's answer: the JSON object's `title`, trimmed of
/// quotes and trailing punctuation, its whitespace collapsed, cut to length.
/// `None` when the answer is not the shape asked for or comes to nothing.
fn clean(answer: &str) -> Option<String> {
    let answer = answer.trim_start();
    if !answer.starts_with('{') {
        return None;
    }
    let title = serde_json::from_str::<Value>(answer).ok()?.get("title")?.as_str()?.to_string();
    let title = title.trim().trim_matches(|c| "\"'`\u{201c}\u{201d}\u{2018}\u{2019}".contains(c));
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = title.trim_end_matches(['.', '?', '!']).trim_end();
    if title.is_empty() {
        return None;
    }
    Some(title.chars().take(MAX_CHARS).collect())
}

/// Shortens `text` to at most `bytes`, on a character boundary.
fn truncate(text: &mut String, bytes: usize) {
    if text.len() <= bytes {
        return;
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}
