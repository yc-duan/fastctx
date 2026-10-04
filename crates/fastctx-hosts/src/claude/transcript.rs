//! Read-only access to Claude Code's versioned, internal transcript format.
//! Each read starts from the file, so a previous read cannot retain rows the
//! host subsequently removed. Turn attribution belongs to the caller.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use fastctx_types::session::{Happened, Item, SessionEvent, ToolStatus, UserMessage, What};
use jiff::Timestamp;
use serde_json::{Value, json};

use super::{content, tools};

/// What the host still stores, with unreadable records reported separately.
#[derive(Debug)]
pub struct Transcript {
    pub events: Vec<SessionEvent>,
    pub problems: Vec<String>,
}

struct Row {
    line: usize,
    value: Value,
}

#[derive(Clone, Copy)]
struct Block {
    row: usize,
    index: usize,
}

#[derive(Default)]
struct Pair {
    calls: Vec<Block>,
    results: Vec<Block>,
}

impl Pair {
    fn readable(&self, rows: &[Row], by_uuid: &HashMap<String, usize>) -> bool {
        if self.calls.len() != 1 || self.results.len() > 1 {
            return false;
        }
        let call = self.calls[0];
        let caller = &rows[call.row];
        if !valid_call(&caller.blocks().expect("indexed blocks")[call.index]) {
            return false;
        }
        let Some(result) = self.results.first() else { return true };
        let result_row = &rows[result.row];
        if !valid_result(&result_row.blocks().expect("indexed blocks")[result.index]) {
            return false;
        }
        let Some(source) = result_row.value.get("sourceToolAssistantUUID").filter(|source| !source.is_null()) else {
            return true;
        };
        let Some(source) = source.as_str() else { return false };
        let Some(&source_index) = by_uuid.get(source) else { return true };
        if source_index == call.row {
            return true;
        }
        // A response can be stored as several UUID-bearing rows. A source
        // UUID may name another block of that same API response.
        let response = caller.value.pointer("/message/id").and_then(Value::as_str).filter(|id| !id.is_empty());
        response.is_some() && response == rows[source_index].value.pointer("/message/id").and_then(Value::as_str)
    }
}

/// Reads the supplied file only; never discovers sibling or subagent files.
///
/// JSONL records are read one line at a time, including a complete final JSON
/// value without a newline. An incomplete tail is left for a subsequent read. Duplicate
/// UUIDs use the last stored form at the original presentation position.
/// Missing times and malformed records produce problems, never invented times.
/// Unknown records without UUIDs are identified by their file line only.
/// Detected size or modification-time changes return `Interrupted`, without a retry.
pub fn read(path: &Path) -> io::Result<Transcript> {
    let file = File::open(path)?;
    let before = file.metadata()?;
    let mut reader = BufReader::new(file);
    let mut rows: Vec<Row> = Vec::new();
    let mut by_uuid = HashMap::new();
    let mut transcript = Transcript { events: Vec::new(), problems: Vec::new() };
    let mut line = Vec::new();
    let mut number = 0;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        number += 1;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let value: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(error) if line.last() != Some(&b'\n') && error.is_eof() => break,
            Err(error) => {
                transcript.problems.push(format!("line {number}: invalid JSON at column {}", error.column()));
                continue;
            }
        };
        let row = Row { line: number, value };
        if !row.value.is_object() || row.kind().is_none() {
            transcript.problem(&row, "expected an object with a string type");
            continue;
        }
        if let Some(sidechain) = row.value.get("isSidechain")
            && !sidechain.is_boolean()
        {
            transcript.problem(&row, "isSidechain is not a boolean");
            continue;
        }
        if row.value.get("agentId").is_some_and(|id| !id.is_null() && !id.is_string()) {
            transcript.problem(&row, "agentId is not a string or null");
            continue;
        }
        if row.value["isSidechain"] == true || row.value.get("agentId").is_some_and(|id| !id.is_null()) {
            continue;
        }
        if row.kind() == Some("progress")
            && matches!(
                row.value.pointer("/data/type").and_then(Value::as_str),
                Some("agent_progress" | "skill_progress")
            )
        {
            continue;
        }
        if bookkeeping(row.kind().expect("type checked")) {
            continue;
        }
        if let Some(uuid) = row.value.get("uuid") {
            let Some(uuid) = uuid.as_str().filter(|uuid| !uuid.is_empty()) else {
                transcript.problem(&row, "uuid is not a nonempty string");
                continue;
            };
            if let Some(&index) = by_uuid.get(uuid) {
                rows[index] = row;
                continue;
            }
            by_uuid.insert(uuid.to_string(), rows.len());
        }
        rows.push(row);
    }
    // Claude Code can truncate and rewrite in place (verified 2026-10-04).
    // Reject a mixed read instead of presenting it as a coherent transcript.
    let after = reader.get_ref().metadata()?;
    let named = path.metadata()?;
    if before.len() != after.len()
        || before.modified()? != after.modified()?
        || after.len() != named.len()
        || after.modified()? != named.modified()?
    {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Claude transcript changed while it was being read"));
    }

    let mut times = Vec::with_capacity(rows.len());
    let mut removed = HashSet::new();
    for row in &rows {
        let at = row.value.get("timestamp").and_then(Value::as_str).and_then(|text| text.parse::<Timestamp>().ok());
        if at.is_none() {
            transcript.problem(row, "missing or invalid timestamp");
        }
        times.push(at);
        let references = if at.is_none() || row.message_problem().is_some() {
            None
        } else if row.kind() == Some("assistant") {
            row.value.get("supersedesUuids")
        } else if row.kind() == Some("system") && row.value["subtype"] == "model_refusal_fallback" {
            row.value.get("retractedMessageUuids")
        } else {
            None
        };
        if let Some(references) = references {
            if let Some(ids) = references.as_array().filter(|ids| ids.iter().all(Value::is_string)) {
                removed.extend(ids.iter().filter_map(Value::as_str));
            } else {
                transcript.problem(row, "invalid retracted message UUID list");
            }
        }
    }
    let visible = |row: &Row| {
        !row.value.get("uuid").and_then(Value::as_str).is_some_and(|id| removed.contains(id))
            && !row.value.get("sourceToolAssistantUUID").and_then(Value::as_str).is_some_and(|id| removed.contains(id))
    };
    let mut pairs: HashMap<&str, Pair> = HashMap::new();
    let mut summaries: HashMap<&str, Vec<usize>> = HashMap::new();
    for (row_index, row) in rows.iter().enumerate().filter(|(index, row)| times[*index].is_some() && visible(row)) {
        if row.message_problem().is_some() {
            continue;
        }
        if row.kind() == Some("user")
            && row.value["isCompactSummary"] == true
            && let Some(parent) = row.value.get("parentUuid").and_then(Value::as_str)
            && let Some(&boundary) = by_uuid.get(parent)
            && rows[boundary].compaction()
            && times[boundary].is_some()
            && visible(&rows[boundary])
        {
            summaries.entry(parent).or_default().push(row_index);
        }
        for (index, block) in row.blocks().into_iter().flatten().enumerate() {
            let reference = Block { row: row_index, index };
            match (row.kind(), block.get("type").and_then(Value::as_str)) {
                (Some("assistant"), Some("tool_use")) => {
                    if let Some(id) = block.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                        pairs.entry(id).or_default().calls.push(reference);
                    }
                }
                (Some("user"), Some("tool_result")) => {
                    if let Some(id) = block.get("tool_use_id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                        pairs.entry(id).or_default().results.push(reference);
                    }
                }
                _ => {}
            }
        }
    }
    let attached: HashSet<usize> = summaries.values().flatten().copied().collect();
    for (index, row) in rows.iter().enumerate() {
        let Some(at) = times[index] else { continue };
        if !visible(row) || attached.contains(&index) {
            continue;
        }
        if let Some(problem) = row.message_problem() {
            transcript.problem(row, problem);
            transcript.other(row, at, 0, "record", row.value.clone());
            continue;
        }
        if row.compaction() || row.kind() == Some("user") && row.value["isCompactSummary"] == true {
            let summary = row.value.get("uuid").and_then(Value::as_str).and_then(|id| summaries.get(id));
            let detail = match summary {
                Some(summary) => {
                    json!({ "boundary": row.value, "summaries": summary.iter().map(|&index| &rows[index].value).collect::<Vec<_>>() })
                }
                None => row.value.clone(),
            };
            transcript.finish(at, row.id(0), at, What::Compaction, detail);
            continue;
        }
        match row.kind() {
            Some("assistant" | "user") => {
                let message = &row.value["message"];
                let prompt = row.kind() == Some("user") && row.prompt();
                if row.kind() == Some("user")
                    && !prompt
                    && !row.blocks().is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
                {
                    transcript.other(row, at, 0, "user", row.value.clone());
                    continue;
                }
                if prompt {
                    let words = match message.get("content") {
                        Some(Value::String(text)) => Some(text.clone()),
                        Some(Value::Array(blocks))
                            if blocks.iter().any(|block| {
                                block["type"] == "image"
                                    || block["type"] == "text" && block.get("text").is_some_and(Value::is_string)
                            }) =>
                        {
                            Some(
                                blocks
                                    .iter()
                                    .filter(|block| block["type"] == "text")
                                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            )
                        }
                        _ => None,
                    };
                    if let Some(text) = words {
                        transcript.events.push(SessionEvent {
                            at,
                            happened: Happened::Heard {
                                delivery: None,
                                turn: None,
                                from: None,
                                text,
                                message: Some(Box::new(row.user_message())),
                            },
                        });
                    }
                }
                match message.get("content") {
                    Some(Value::String(_)) if row.kind() == Some("user") => {
                        if !prompt {
                            transcript.other(row, at, 0, "user", row.value.clone());
                        }
                    }
                    Some(Value::Array(blocks)) => {
                        for (block_index, block) in blocks.iter().enumerate() {
                            let kind = block.get("type").and_then(Value::as_str);
                            let what = match (row.kind(), kind) {
                                (Some("assistant"), Some("text")) => block
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .map(|text| What::Message { text: text.to_string() }),
                                (Some("assistant"), Some("thinking")) => block
                                    .get("thinking")
                                    .and_then(Value::as_str)
                                    .map(|text| What::Thinking { text: text.to_string() }),
                                (Some("assistant"), Some("redacted_thinking")) => {
                                    if block.get("data").is_some_and(Value::is_string) {
                                        Some(What::Thinking { text: String::new() })
                                    } else {
                                        transcript.problem(row, "redacted thinking block has no string data");
                                        None
                                    }
                                }
                                (Some("assistant"), Some("tool_use")) => {
                                    let pair = block.get("id").and_then(Value::as_str).and_then(|id| pairs.get(id));
                                    if let Some(pair) = pair.filter(|pair| pair.readable(&rows, &by_uuid)) {
                                        let result = pair.results.first().map(|result| {
                                            (&rows[result.row], result.index, times[result.row].expect("indexed time"))
                                        });
                                        transcript.tool(row, at, block, result);
                                        continue;
                                    }
                                    transcript.problem(row, "invalid or ambiguous tool_use");
                                    None
                                }
                                (Some("user"), Some("tool_result")) => {
                                    let pair =
                                        block.get("tool_use_id").and_then(Value::as_str).and_then(|id| pairs.get(id));
                                    if pair.is_some_and(|pair| pair.readable(&rows, &by_uuid)) {
                                        continue;
                                    }
                                    transcript.problem(row, "invalid tool_result or no unambiguous valid tool_use");
                                    None
                                }
                                (Some("user"), Some("text")) if prompt => {
                                    if block.get("text").is_some_and(Value::is_string) {
                                        continue;
                                    } else {
                                        transcript.problem(row, "text block has no string text");
                                    }
                                    None
                                }
                                (Some("user"), Some("image" | "document")) if prompt => continue,
                                _ => {
                                    if kind.is_none() {
                                        transcript.problem(row, "content block has no string type");
                                    }
                                    None
                                }
                            };
                            if let Some(what) = what {
                                transcript.finish(at, row.id(block_index), at, what, block.clone());
                            } else {
                                if row.kind() == Some("assistant") && matches!(kind, Some("text" | "thinking")) {
                                    transcript.problem(row, "prose block has no string body");
                                }
                                transcript.other(row, at, block_index, kind.unwrap_or("block"), block.clone());
                            }
                        }
                    }
                    _ => {
                        transcript.problem(row, "message content is not a supported string or block array");
                        transcript.other(row, at, 0, "record", row.value.clone());
                    }
                }
            }
            _ => transcript.other(row, at, 0, row.kind().expect("type checked"), row.value.clone()),
        }
    }
    Ok(transcript)
}

impl Row {
    fn user_message(&self) -> UserMessage {
        let content = match &self.value["message"]["content"] {
            Value::Array(blocks) => blocks.clone(),
            Value::String(text) => vec![json!({ "type": "text", "text": text })],
            _ => Vec::new(),
        };
        UserMessage {
            id: self.value["uuid"].as_str().expect("validated uuid").to_string(),
            client_id: None,
            content,
            hook_fragments: Vec::new(),
            presentation: Value::Null,
            detail: self.value.clone(),
        }
    }

    fn message_problem(&self) -> Option<&'static str> {
        if !matches!(self.kind(), Some("user" | "assistant")) {
            return None;
        }
        if self.value.get("uuid").and_then(Value::as_str).is_none() {
            return Some("message has no uuid");
        }
        let message = self.value.get("message").filter(|message| message.is_object());
        let Some(message) = message else { return Some("missing or invalid message") };
        if message.get("role").is_some_and(|role| role.as_str() != self.kind())
            || ["isMeta", "isCompactSummary", "isVisibleInTranscriptOnly", "isVirtual", "turnCompanion"]
                .iter()
                .any(|key| self.value.get(*key).is_some_and(|value| !value.is_boolean()))
            || self
                .value
                .get("origin")
                .filter(|value| !value.is_null())
                .is_some_and(|origin| origin.get("kind").and_then(Value::as_str).is_none())
        {
            return Some("invalid message role, flag or origin");
        }
        match message.get("content") {
            Some(Value::Array(_)) => None,
            Some(Value::String(_)) if self.kind() == Some("user") => None,
            _ => Some("message content is not a supported string or block array"),
        }
    }

    fn kind(&self) -> Option<&str> {
        self.value.get("type").and_then(Value::as_str)
    }

    fn blocks(&self) -> Option<&Vec<Value>> {
        self.value.get("message")?.get("content")?.as_array()
    }

    fn id(&self, index: usize) -> String {
        match self.value.get("uuid").and_then(Value::as_str) {
            Some(uuid) => serde_json::to_string(&(uuid, index)),
            None => serde_json::to_string(&("transcript-line", self.line, index)),
        }
        .expect("a source coordinate serializes")
    }

    fn compaction(&self) -> bool {
        self.kind() == Some("system")
            && matches!(
                self.value.get("subtype").and_then(Value::as_str),
                Some("compact_boundary" | "microcompact_boundary")
            )
    }

    fn prompt(&self) -> bool {
        if ["isMeta", "isCompactSummary", "isVisibleInTranscriptOnly", "isVirtual", "turnCompanion"]
            .iter()
            .any(|key| self.value[*key] == true)
            || ["sourceToolAssistantUUID", "sourceToolUseID", "toolUseResult"]
                .iter()
                .any(|key| self.value.get(*key).is_some_and(|value| !value.is_null()))
            || self.blocks().is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
        {
            return false;
        }
        if self.value["promptSource"] == "system"
            || matches!(
                self.value.get("turnOrigin").and_then(Value::as_str),
                Some("system" | "host_synthetic" | "auto_continuation" | "task_notification" | "scheduled")
            )
        {
            return false;
        }
        let first = self.value["message"]["content"]
            .as_str()
            .or_else(|| {
                self.blocks()?
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .find(|text| !text.trim().is_empty())
            })
            .unwrap_or("");
        if [
            "<local-command-caveat>",
            "<local-command-stdout>",
            "<local-command-stderr>",
            "<command-name>",
            "<command-message>",
            "<system-reminder>",
            "<task-notification>",
            "[Request interrupted by user",
        ]
        .iter()
        .any(|prefix| first.trim_start().starts_with(prefix))
        {
            return false;
        }
        self.value.get("origin").filter(|origin| !origin.is_null()).is_none_or(|origin| {
            matches!(
                origin.get("kind").and_then(Value::as_str),
                Some("human" | "plugin" | "peer" | "channel" | "coordinator" | "slack-ping")
            )
        })
    }
}

impl Transcript {
    fn problem(&mut self, row: &Row, reason: &str) {
        self.problems.push(format!("line {}: {reason}", row.line));
    }

    fn finish(&mut self, at: Timestamp, id: String, started_at: Timestamp, what: What, detail: Value) {
        self.events.push(SessionEvent {
            at,
            happened: Happened::Finished {
                turn: String::new(),
                item: Item { id, started_at: Some(started_at), detail: Some(detail), what },
            },
        });
    }

    fn other(&mut self, row: &Row, at: Timestamp, index: usize, kind: &str, detail: Value) {
        self.finish(at, row.id(index), at, What::Other { host_kind: kind.to_string() }, detail);
    }

    fn tool(&mut self, row: &Row, at: Timestamp, request: &Value, result: Option<(&Row, usize, Timestamp)>) {
        let (ended, output, status, stored) = match result {
            Some((result_row, index, ended)) => {
                let block = &result_row.blocks().expect("indexed blocks")[index];
                let status = if block["is_error"] == true { ToolStatus::Failed } else { ToolStatus::Done };
                (
                    ended,
                    content::content(&block["content"]),
                    status,
                    json!({ "uuid": result_row.value.get("uuid"), "block": block, "toolUseResult": result_row.value.get("toolUseResult"), "sourceToolAssistantUUID": result_row.value.get("sourceToolAssistantUUID") }),
                )
            }
            None => (at, Vec::new(), ToolStatus::Unknown, Value::Null),
        };
        let tool = tools::tool(
            request["name"].as_str().expect("validated name"),
            request["input"].clone(),
            output,
            status,
            None,
        );
        self.finish(
            ended,
            request["id"].as_str().expect("indexed tool id").to_string(),
            at,
            What::Tool(Box::new(tool)),
            json!({ "requested": request, "stored": stored, "uuid": row.value.get("uuid") }),
        );
    }
}

fn valid_call(block: &Value) -> bool {
    block.get("name").and_then(Value::as_str).is_some_and(|name| !name.is_empty())
        && block.get("input").is_some_and(Value::is_object)
}

fn valid_result(block: &Value) -> bool {
    block.get("is_error").is_none_or(Value::is_boolean)
        && matches!(block.get("content"), Some(Value::String(_) | Value::Array(_) | Value::Null) | None)
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
