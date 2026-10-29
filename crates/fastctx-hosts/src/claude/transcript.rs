//! Read-only access to Claude Code's versioned, internal transcript format.
//! Each read starts from the file, so a previous read cannot retain rows the
//! host subsequently removed. Turn attribution belongs to the caller.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use fastctx_types::session::{
    Does, Happened, HostAgentStart, Item, SessionChange, SessionEvent, ToolStatus, UserMessage, What,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{content, subagents, tools};

/// What the host still stores, with unreadable records reported separately.
#[derive(Debug)]
pub struct Transcript {
    pub title: Option<String>,
    pub pending_cuts: bool,
    pub retained_tail: Option<String>,
    pub tail_uuid: Option<String>,
    pub context_uuids: Vec<String>,
    pub context_parent: Option<String>,
    pub context_tail: Option<String>,
    pub context_times: Vec<Timestamp>,
    pub context_messages: Vec<Value>,
    /// An idle native fork may close a trailing user summary without a model request.
    pub synthetic_tail: bool,
    pub events: Vec<SessionEvent>,
    pub problems: Vec<String>,
    /// Every record's UUID the file holds, shown or not: whether a record
    /// the session is known to have kept has reached the file yet.
    pub uuids: HashSet<String>,
    /// The agents the session started, as the files beside its record tell
    /// them.
    beside: subagents::Beside,
    /// The agents its Agent calls started, by their ids.
    agents: HashMap<String, HostAgentStart>,
}

pub fn same_projection(left: &Value, right: &Value) -> bool {
    super::context::same(left, right)
}

impl Transcript {
    /// Confirms the mod's public tail before assigning native row UUIDs to it.
    pub fn message_tail_matches(&self, tail: &[Value]) -> bool {
        tail.len() <= self.context_messages.len()
            && tail
                .iter()
                .zip(self.context_messages.iter().skip(self.context_messages.len().saturating_sub(tail.len())))
                .all(|(expected, actual)| super::context::same(expected, actual))
    }
}

/// A World context replacement, identified by observed message UUIDs and the
/// native compact boundary's logical parent. The host file is never edited.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryCut {
    pub until: String,
    pub parent: String,
    pub messages: usize,
    pub retained: Vec<Value>,
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

/// Reads the session's own record, and of what is beside it only what tells
/// of the agents it started (`subagents`): never their records, which are
/// read one at a time as the user opens one (`read_agent`).
///
/// JSONL records are read one line at a time, including a complete final JSON
/// value without a newline. An incomplete tail is left for a subsequent read. Duplicate
/// UUIDs use the last stored form at the original presentation position.
/// Missing times and malformed records produce problems, never invented times.
/// Unknown records without UUIDs are identified by their file line only.
/// Detected size or modification-time changes return `Interrupted`, without a retry.
pub fn read(path: &Path) -> io::Result<Transcript> {
    read_with_cuts(path, &[])
}

pub fn read_with_cuts(path: &Path, cuts: &[HistoryCut]) -> io::Result<Transcript> {
    read_rows(path, cuts, None, &HashSet::new())
}

/// Reads a session that continues its conversation (`background::continue_args`):
/// the records Claude Code copied there, which keep their ids, give no
/// events when one of `earlier`, the ids the conversation's sessions before
/// it hold, names them, as that session's own record shows them already.
/// They are still the session's context.
pub fn read_continuing(path: &Path, cuts: &[HistoryCut], earlier: &HashSet<String>) -> io::Result<Transcript> {
    read_rows(path, cuts, None, earlier)
}

/// Reads the record of one agent the session started (`subagents::record`):
/// the agent's own rows, as a conversation of its own whose first words are
/// what it was asked.
pub fn read_agent(path: &Path, agent: &str) -> io::Result<Transcript> {
    read_rows(path, &[], Some(agent), &HashSet::new())
}

fn read_rows(
    path: &Path,
    cuts: &[HistoryCut],
    agent: Option<&str>,
    earlier: &HashSet<String>,
) -> io::Result<Transcript> {
    let file = File::open(path)?;
    let before = file.metadata()?;
    let mut reader = BufReader::new(file);
    let mut rows: Vec<Row> = Vec::new();
    let mut by_uuid = HashMap::new();
    let mut transcript = Transcript {
        title: None,
        pending_cuts: false,
        retained_tail: None,
        tail_uuid: None,
        context_uuids: Vec::new(),
        context_parent: None,
        context_tail: None,
        context_times: Vec::new(),
        context_messages: Vec::new(),
        synthetic_tail: false,
        events: Vec::new(),
        problems: Vec::new(),
        uuids: HashSet::new(),
        beside: if agent.is_none() { subagents::beside(path) } else { subagents::Beside::default() },
        agents: HashMap::new(),
    };
    let mut titles = super::titles::Titles::default();
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
        if matches!(row.kind(), Some("custom-title" | "ai-title")) {
            if let Err(error) = titles.row(&row.value) {
                transcript.problem(&row, &error.to_string());
            }
            continue;
        }
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
        let own = match agent {
            None => row.value["isSidechain"] != true && row.value.get("agentId").is_none_or(Value::is_null),
            Some(agent) => row.value.get("agentId").and_then(Value::as_str) == Some(agent),
        };
        if !own {
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
    transcript.uuids = by_uuid.keys().cloned().collect();
    transcript.title = titles.title();
    transcript.tail_uuid = rows
        .iter()
        .rev()
        .find(|row| matches!(row.kind(), Some("user" | "assistant")))
        .and_then(|row| row.value["uuid"].as_str())
        .map(str::to_owned);
    let context_start = rows.iter().rposition(Row::replacement).map_or(0, |index| index + 1);
    transcript.context_parent = context_start
        .checked_sub(1)
        .and_then(|index| rows[index].value["logicalParentUuid"].as_str())
        .map(str::to_owned);
    let context_order = super::context::order(&rows.iter().map(|row| &row.value).collect::<Vec<_>>())?;
    transcript.context_tail = context_order
        .iter()
        .rev()
        .map(|&index| &rows[index])
        .find_map(|row| row.value["uuid"].as_str())
        .map(str::to_owned);

    transcript.synthetic_tail = context_order
        .iter()
        .rev()
        .map(|&index| &rows[index])
        .find(|row| matches!(row.kind(), Some("user" | "assistant")))
        .is_some_and(|row| {
            row.kind() == Some("assistant")
                && row.value.pointer("/message/model").and_then(Value::as_str) == Some("<synthetic>")
                && row.value.pointer("/message/content")
                    == Some(&json!([{"type":"text","text":"No response requested."}]))
        });

    let mut cut_rows = HashSet::new();
    for cut in cuts {
        if cut.retained.len() != cut.messages {
            return Err(io::Error::other("the World rewind omitted its retained message projections"));
        }
        let Some(&until) = by_uuid.get(&cut.until) else {
            transcript.problems.push("the retained World turn's transcript anchor is absent".into());
            continue;
        };
        let boundaries: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.replacement() && row.value["logicalParentUuid"].as_str() == Some(&cut.parent))
            .map(|(index, _)| index)
            .collect();
        if boundaries.is_empty() {
            transcript.pending_cuts = true;
            continue;
        }
        if boundaries.len() != 1 {
            return Err(io::Error::other("the World rewind has an ambiguous compact boundary"));
        }
        let boundary = boundaries[0];
        if boundary <= until {
            return Err(io::Error::other("the World rewind boundary precedes its retained anchor"));
        }
        let copied: Vec<_> = rows
            .iter()
            .enumerate()
            .skip(boundary + 1)
            .take_while(|(_, row)| !row.replacement())
            .filter(|(_, row)| super::context::projected(&row.value))
            .take(cut.messages)
            .map(|(index, _)| index)
            .collect();
        if copied.len() != cut.messages {
            transcript.pending_cuts = true;
            continue;
        }
        if !copied
            .iter()
            .zip(&cut.retained)
            .all(|(&index, expected)| super::context::same(&super::context::projection(&rows[index].value), expected))
        {
            return Err(io::Error::other("Claude persisted a different replacement prefix than its mod returned"));
        }
        let tail = copied.last().copied().unwrap_or(boundary);
        transcript.retained_tail = rows[tail].value["uuid"].as_str().map(str::to_owned);
        cut_rows.extend(until + 1..boundary);
        cut_rows.extend(boundary + 1..=tail);
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
    let native_visible = |row: &Row| {
        !row.value.get("uuid").and_then(Value::as_str).is_some_and(|id| removed.contains(id))
            && !row.value.get("sourceToolAssistantUUID").and_then(Value::as_str).is_some_and(|id| removed.contains(id))
    };
    transcript.context_uuids = context_order
        .iter()
        .map(|&index| &rows[index])
        .filter(|row| native_visible(row) && super::context::projected(&row.value))
        .filter_map(|row| row.value["uuid"].as_str().map(str::to_owned))
        .collect();
    transcript.context_times = context_order
        .iter()
        .map(|&index| &rows[index])
        .filter(|row| native_visible(row) && super::context::projected(&row.value))
        .filter_map(|row| row.value["timestamp"].as_str()?.parse().ok())
        .collect();
    transcript.context_messages = context_order
        .iter()
        .map(|&index| &rows[index])
        .filter(|row| native_visible(row) && super::context::projected(&row.value))
        .map(|row| super::context::projection(&row.value))
        .collect();
    let visible = |row: &Row| {
        native_visible(row)
            && !row
                .value
                .get("uuid")
                .and_then(Value::as_str)
                .and_then(|id| by_uuid.get(id))
                .is_some_and(|index| cut_rows.contains(index))
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
        if row.value.get("uuid").and_then(Value::as_str).is_some_and(|uuid| earlier.contains(uuid)) {
            continue;
        }
        if let Some(problem) = row.message_problem() {
            transcript.problem(row, problem);
            transcript.other(row, at, 0, "record", row.value.clone());
            continue;
        }
        if row.kind() == Some("assistant")
            && let Some(model) = row.value["message"]["model"].as_str()
        {
            let items = row
                .blocks()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, block)| {
                    if block["type"] == "tool_use" {
                        block["id"].as_str().map(str::to_owned).unwrap_or_else(|| row.id(index))
                    } else {
                        row.id(index)
                    }
                })
                .collect();
            transcript.events.push(SessionEvent {
                at,
                happened: Happened::Changed {
                    turn: None,
                    change: SessionChange::MessageUsage {
                        items,
                        response: row.value["message"]["id"].as_str().map(str::to_owned),
                        model: model.to_owned(),
                        api_error: row.value["isApiErrorMessage"] == true,
                        usage: row.value["message"]["usage"].clone(),
                    },
                },
            });
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
                    transcript.hidden(row, at);
                    continue;
                }
                if prompt && transcript.agent_told(row, at) {
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
                                notice: None,
                            },
                        });
                    }
                }
                match message.get("content") {
                    Some(Value::String(_)) if prompt => {}
                    Some(Value::Array(blocks)) => {
                        // How long the model thought, as Claude Code measured
                        // it: one figure for the row, so only a row with one
                        // block of thinking can give it to that block.
                        let thinking = |block: &Value| {
                            matches!(block.get("type").and_then(Value::as_str), Some("thinking" | "redacted_thinking"))
                        };
                        let thought = row
                            .value
                            .get("thinkingDurationMs")
                            .and_then(Value::as_u64)
                            .filter(|_| blocks.iter().filter(|block| thinking(block)).count() == 1);
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
                                    .map(|text| What::Thinking { text: text.to_string(), duration_ms: thought }),
                                (Some("assistant"), Some("redacted_thinking")) => {
                                    if block.get("data").is_some_and(Value::is_string) {
                                        Some(What::Thinking { text: String::new(), duration_ms: thought })
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
            Some("attachment" | "system" | "queue-operation") => transcript.hidden(row, at),
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
            attachments: Vec::new(),
            images: Vec::new(),
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
            || [
                "isMeta",
                "isCompactSummary",
                "isVisibleInTranscriptOnly",
                "isVirtual",
                "turnCompanion",
                "dispatchEcho",
                "stackedExpansion",
            ]
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

    fn replacement(&self) -> bool {
        self.kind() == Some("system") && self.value["subtype"] == "compact_boundary"
    }

    fn prompt(&self) -> bool {
        if [
            "isCompactSummary",
            "isVisibleInTranscriptOnly",
            "isVirtual",
            "turnCompanion",
            "dispatchEcho",
            "stackedExpansion",
        ]
        .iter()
        .any(|key| self.value[*key] == true)
            || ["sourceToolAssistantUUID", "sourceToolUseID", "toolUseResult"]
                .iter()
                .any(|key| self.value.get(*key).is_some_and(|value| !value.is_null()))
            || self.blocks().is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
        {
            return false;
        }
        // System-origin submissions can be prompts, including a plugin speaking
        // as the user. Meta prompts carry input provenance too; a plugin's
        // session.append context note carries origin alone.
        if self.value["isMeta"] == true
            && !["promptSource", "turnOrigin"].iter().any(|key| self.value.get(*key).is_some_and(Value::is_string))
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
            "[Request interrupted by user",
        ]
        .iter()
        .any(|prefix| first.trim_start().starts_with(prefix))
        {
            return false;
        }
        true
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
                item: Item { draft: None, id, started_at: Some(started_at), detail: Some(detail), what },
            },
        });
    }

    fn other(&mut self, row: &Row, at: Timestamp, index: usize, kind: &str, detail: Value) {
        self.finish(at, row.id(index), at, What::Other { host_kind: kind.to_string() }, detail);
    }

    fn hidden(&mut self, row: &Row, at: Timestamp) {
        self.finish(
            at,
            row.id(0),
            at,
            What::Hidden { host_kind: row.kind().expect("type checked").to_string() },
            row.value.clone(),
        );
    }

    fn tool(&mut self, row: &Row, at: Timestamp, request: &Value, result: Option<(&Row, usize, Timestamp)>) {
        let answer = result.and_then(|(result_row, ..)| result_row.value.get("toolUseResult"));
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
        let name = request["name"].as_str().expect("validated name");
        let call = request["id"].as_str().expect("indexed tool id").to_string();
        let mut tool = tools::tool(name, request["input"].clone(), output, status, None);
        // An Agent call tells of the agent it started; what the agent did is
        // in its own record (`read_agent`).
        let agent = matches!(name, "Agent" | "Task")
            .then(|| subagents::of_call(&call, &request["input"], answer, status == ToolStatus::Failed, &self.beside))
            .flatten();
        if let (Some((start, _)), Does::Agent { agents, .. }) = (&agent, &mut tool.does) {
            *agents = vec![start.id.clone()];
        }
        self.finish(
            ended,
            call,
            at,
            What::Tool(Box::new(tool)),
            json!({ "requested": request, "stored": stored, "uuid": row.value.get("uuid") }),
        );
        if let Some((start, end)) = agent {
            let id = start.id.clone();
            self.agents.insert(id.clone(), start.clone());
            self.changed(ended, SessionChange::AgentStarted { agent: Box::new(start) });
            if let Some(end) = end {
                self.changed(ended, SessionChange::AgentEnded { agent_id: id, end });
            }
        }
    }

    fn changed(&mut self, at: Timestamp, change: SessionChange) {
        self.events.push(SessionEvent { at, happened: Happened::Changed { turn: None, change } });
    }

    /// A row Claude Code wrote to tell the session that one of its agents
    /// that went on in the background ended: the agent's end, told here.
    fn agent_told(&mut self, row: &Row, at: Timestamp) -> bool {
        if row.value.pointer("/origin/kind").and_then(Value::as_str) != Some("task-notification") {
            return false;
        }
        let text = match &row.value["message"]["content"] {
            Value::String(text) => text.clone(),
            Value::Array(blocks) => {
                blocks.iter().filter_map(|block| block.get("text").and_then(Value::as_str)).collect()
            }
            _ => return false,
        };
        let Some(mut notified) = subagents::notified(&text) else { return false };
        let known = self.agents.get(&notified.task).map(|start| start.task.clone());
        let beside = self.beside.ids().any(|id| id == notified.task);
        if known.is_none() && !beside {
            return false;
        }
        notified.end.task = known.flatten();
        self.changed(at, SessionChange::AgentEnded { agent_id: notified.task, end: notified.end });
        self.hidden(row, at);
        true
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
