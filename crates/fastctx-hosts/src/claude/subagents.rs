//! What Claude Code keeps of the agents a session started (its subagents):
//! each has a record of its own beside the session's, in
//! `<session record without .jsonl>/subagents/agent-<id>.jsonl`, with a
//! small `agent-<id>.meta.json` naming the Agent tool call that started it.
//! The session's own record tells the rest: the Agent call's answer names
//! the agent and, for one that ran to its end before answering, how it
//! went; one that went on in the background is told of later in a
//! `<task-notification>` row (Claude Code 2.1.231 to 2.1.296, read from
//! sessions on disk; its record format is internal, `world.md`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fastctx_types::session::{HostAgentEnd, HostAgentStart, TurnHow};
use serde::Deserialize;
use serde_json::Value;

/// An agent as its meta file beside the session's record tells it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub id: String,
    pub kind: Option<String>,
    pub task: Option<String>,
    pub background: bool,
}

/// The agents a session started, as the files beside its record tell them,
/// by the Agent tool call that started each.
#[derive(Clone, Debug, Default)]
pub struct Beside {
    by_call: HashMap<String, Meta>,
}

impl Beside {
    pub fn of_call(&self, call: &str) -> Option<&Meta> {
        self.by_call.get(call)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.by_call.values().map(|meta| meta.id.as_str())
    }

    /// The meta files' sizes and times, which change as agents start.
    pub fn len(&self) -> usize {
        self.by_call.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_call.is_empty()
    }
}

/// Where Claude Code keeps the records of the agents the session whose
/// record is `transcript` started.
pub fn directory(transcript: &Path) -> Option<PathBuf> {
    let stem = transcript.file_stem()?;
    Some(transcript.with_file_name(stem).join("subagents"))
}

/// Whether `id` can be an agent's id as Claude Code writes them, and so a
/// file name: it names a file here, so nothing but letters, digits, `-`
/// and `_` passes.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// The record of the agent `id` the session whose record is `transcript`
/// started; none for an id that cannot name one.
pub fn record(transcript: &Path, id: &str) -> Option<PathBuf> {
    valid_id(id).then(|| directory(transcript)).flatten().map(|directory| directory.join(format!("agent-{id}.jsonl")))
}

/// The agents beside the session's record whose record is `transcript`.
/// A meta file that cannot be read is left out: the session's own record
/// still tells of its agent once its call answers.
pub fn beside(transcript: &Path) -> Beside {
    let mut found = Beside::default();
    let Some(directory) = directory(transcript) else { return found };
    let Ok(entries) = std::fs::read_dir(&directory) else { return found };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) =
            name.to_str().and_then(|name| name.strip_prefix("agent-")).and_then(|name| name.strip_suffix(".meta.json"))
        else {
            continue;
        };
        if let Some((call, meta)) = read_meta(&entry.path(), id) {
            found.by_call.insert(call, meta);
        }
    }
    found
}

/// What the meta file beside an agent's record (`record`) tells of it, and
/// the Agent call that started it.
pub fn meta(record: &Path, id: &str) -> Option<(String, Meta)> {
    read_meta(&record.with_file_name(format!("agent-{id}.meta.json")), id)
}

/// A meta file, and the Agent call it names.
fn read_meta(path: &Path, id: &str) -> Option<(String, Meta)> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct File {
        tool_use_id: Option<String>,
        agent_type: Option<String>,
        description: Option<String>,
        request_shape: Option<String>,
    }
    if !valid_id(id) {
        return None;
    }
    let file = serde_json::from_slice::<File>(&std::fs::read(path).ok()?).ok()?;
    let call = file.tool_use_id.filter(|call| !call.is_empty())?;
    Some((
        call,
        Meta {
            id: id.to_string(),
            kind: file.agent_type,
            task: file.description,
            background: file.request_shape.as_deref() == Some("background"),
        },
    ))
}

/// What an Agent call tells of the agent it started: its own input, the
/// record of its answer (`toolUseResult`), and the meta file beside the
/// session's record when the call has not answered yet. The end is told
/// for an agent that ran to its end before the call answered.
pub fn of_call(
    call: &str,
    input: &Value,
    answer: Option<&Value>,
    failed: bool,
    beside: &Beside,
) -> Option<(HostAgentStart, Option<HostAgentEnd>)> {
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    let answer = answer.filter(|answer| answer.is_object());
    let meta = beside.of_call(call);
    let id = answer.and_then(|answer| text(answer, "agentId")).or_else(|| meta.map(|meta| meta.id.clone()))?;
    if !valid_id(&id) {
        return None;
    }
    let start = HostAgentStart {
        id,
        call: Some(call.to_string()),
        parent: None,
        kind: text(input, "subagent_type")
            .or_else(|| answer.and_then(|answer| text(answer, "agentType")))
            .or_else(|| meta.and_then(|meta| meta.kind.clone())),
        name: text(input, "name"),
        task: text(input, "description").or_else(|| meta.and_then(|meta| meta.task.clone())),
        prompt: text(input, "prompt").or_else(|| answer.and_then(|answer| text(answer, "prompt"))),
        model: answer.and_then(|answer| text(answer, "resolvedModel")),
        background: input.get("run_in_background").and_then(Value::as_bool) == Some(true)
            || answer.and_then(|answer| answer.get("isAsync")).and_then(Value::as_bool) == Some(true)
            || meta.is_some_and(|meta| meta.background),
        live: false,
    };
    let status = answer.and_then(|answer| text(answer, "status"));
    let end = match status.as_deref() {
        Some("completed") => Some(HostAgentEnd {
            how: TurnHow::Completed,
            reason: None,
            duration_ms: answer.and_then(|answer| answer.get("totalDurationMs")).and_then(Value::as_u64),
            tokens: answer.and_then(|answer| answer.get("totalTokens")).and_then(Value::as_u64),
            tools: answer.and_then(|answer| answer.get("totalToolUseCount")).and_then(Value::as_u64),
            answer: answer.and_then(|answer| answer.get("content")).map(words).filter(|words| !words.is_empty()),
            task: None,
            told: false,
        }),
        // Launched to go on in the background: its end is told later.
        Some("async_launched" | "teammate_spawned") => None,
        _ if failed => Some(HostAgentEnd {
            how: TurnHow::Failed,
            reason: None,
            duration_ms: None,
            tokens: None,
            tools: None,
            answer: None,
            task: None,
            told: false,
        }),
        _ => None,
    };
    Some((start, end))
}

/// A `<task-notification>` Claude Code wrote into the session for a task
/// that went on in the background: which task, how it ended, and what it
/// reported.
#[derive(Debug, PartialEq, Eq)]
pub struct Notified {
    pub task: String,
    pub call: Option<String>,
    pub end: HostAgentEnd,
}

/// Reads a notification's words: tags Claude Code writes for the model,
/// with figures under `<usage>`. None for anything else.
pub fn notified(text: &str) -> Option<Notified> {
    let text = text.trim();
    let body = text.strip_prefix("<task-notification>")?;
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = body.find(&open)? + open.len();
        let end = body[start..].find(&close)? + start;
        Some(body[start..end].trim().to_string())
    };
    let number = |usage: &str, name: &str| {
        let open = format!("<{name}>");
        let start = usage.find(&open)? + open.len();
        usage[start..].split('<').next()?.trim().parse::<u64>().ok()
    };
    let task = tag("task-id").filter(|task| !task.is_empty())?;
    let usage = tag("usage").unwrap_or_default();
    let how = match tag("status").as_deref() {
        Some("completed") => TurnHow::Completed,
        Some("failed") => TurnHow::Failed,
        Some("stopped" | "killed") => TurnHow::Interrupted,
        Some(other) => TurnHow::Other(other.to_string()),
        None => TurnHow::Other(String::new()),
    };
    Some(Notified {
        task,
        call: tag("tool-use-id").filter(|call| !call.is_empty()),
        end: HostAgentEnd {
            reason: matches!(how, TurnHow::Failed).then(|| tag("summary")).flatten(),
            how,
            duration_ms: number(&usage, "duration_ms"),
            tokens: number(&usage, "subagent_tokens").or_else(|| number(&usage, "total_tokens")),
            tools: number(&usage, "tool_uses"),
            answer: tag("result").filter(|result| !result.is_empty()),
            task: None,
            told: true,
        },
    })
}

/// The words of an answer's content: its text blocks, or the string it is.
fn words(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}
