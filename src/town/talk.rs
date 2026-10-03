//! Watching a resident live and speaking to it as the user (`specs/world-colleagues.md`,
//! "用户在任何地方和居民说话").
//!
//! Whoever watches a resident gets its history first, then every event as it happens: text as
//! the model writes it, each tool as it starts and ends, each turn's start and end, and the
//! words that went in. Events are not stored and not queued for absent watchers: someone who
//! comes back reads the history again.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// One thing that happened in a resident's session, for whoever is watching it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum TalkEvent {
    /// Words that went into the session: the user's own, another resident's, or the World's.
    Words { from: String, text: String },
    TurnStarted { turn: String },
    /// A piece of the text the model is writing.
    Delta { item: String, text: String },
    /// A finished message of the model.
    Message { item: String, text: String },
    /// A tool started (`done` false) or ended (`done` true).
    Tool { item: String, name: String, summary: String, done: bool, failed: bool },
    TurnEnded { turn: String, status: String },
    /// Its state changed: busy, idle, asleep, left.
    State { state: String },
    /// Everything before this came from the history; everything after happens now.
    Live,
    /// The watched resident does not exist here, or the watch cannot continue.
    Gone { reason: String },
}

/// What a watcher already got from the history. A watch registers before it reads the
/// history, so whatever happened while the history was read arrives a second time live; these
/// keys recognize it: words already shown (each once), anything of an item the history shows
/// complete, and the start or end of a turn the history shows ended.
#[derive(Default)]
pub(crate) struct HistoryKeys {
    items: HashSet<String>,
    ended: HashSet<String>,
    words: HashMap<(String, String), usize>,
}

impl HistoryKeys {
    pub(crate) fn of(history: &[TalkEvent]) -> Self {
        let mut keys = Self::default();
        for event in history {
            match event {
                TalkEvent::Message { item, .. } | TalkEvent::Tool { item, done: true, .. } => {
                    keys.items.insert(item.clone());
                }
                TalkEvent::TurnEnded { turn, .. } => {
                    keys.ended.insert(turn.clone());
                }
                TalkEvent::Words { from, text } => *keys.words.entry((from.clone(), text.clone())).or_default() += 1,
                _ => {}
            }
        }
        keys
    }

    /// Whether a live event only repeats something the history already showed.
    pub(crate) fn repeats(&mut self, event: &TalkEvent) -> bool {
        match event {
            TalkEvent::Message { item, .. } | TalkEvent::Tool { item, .. } | TalkEvent::Delta { item, .. } => self.items.contains(item),
            TalkEvent::TurnStarted { turn } | TalkEvent::TurnEnded { turn, .. } => self.ended.contains(turn),
            TalkEvent::Words { from, text } => match self.words.get_mut(&(from.clone(), text.clone())) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// Turns a Codex item into the events a watcher sees; `done` tells a completed item from one
/// that just started. Items without anything to show (reasoning summaries are not shown) give
/// nothing.
pub(crate) fn codex_item(item: &Value, done: bool) -> Option<TalkEvent> {
    let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
    let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let failed = matches!(item.get("status").and_then(Value::as_str), Some("failed" | "declined"))
        || item.get("error").is_some_and(|error| !error.is_null());
    match kind {
        "agentMessage" if done => Some(TalkEvent::Message { item: id, text: text("text") }),
        "agentMessage" => None,
        "userMessage" if done => {
            let words: Vec<&str> = item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect();
            let (from, text) = speaker(&words.join("\n"));
            Some(TalkEvent::Words { from, text })
        }
        "userMessage" => None,
        "commandExecution" => Some(TalkEvent::Tool { item: id, name: "shell".into(), summary: first_line(&text("command")), done, failed }),
        "mcpToolCall" => {
            let name = format!("{}.{}", text("server"), text("tool"));
            let summary = item.get("arguments").map(arguments_summary).unwrap_or_default();
            Some(TalkEvent::Tool { item: id, name, summary, done, failed })
        }
        "fileChange" => {
            let files: Vec<String> = item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|change| change.get("path").and_then(Value::as_str).map(str::to_string))
                .collect();
            Some(TalkEvent::Tool { item: id, name: "apply_patch".into(), summary: files.join(", "), done, failed })
        }
        "webSearch" => Some(TalkEvent::Tool { item: id, name: "web_search".into(), summary: text("query"), done, failed }),
        "" | "reasoning" | "contextCompaction" => None,
        other => Some(TalkEvent::Tool { item: id, name: other.to_string(), summary: String::new(), done, failed }),
    }
}

/// A Codex thread's recorded history as events, oldest first. `running` is the turn the
/// app-server is running now: its end is not on disk yet, and `thread/read` reports such a
/// turn as interrupted, so it is left open.
pub(crate) fn codex_history(thread: &Value, running: Option<&str>) -> Vec<TalkEvent> {
    let mut events = Vec::new();
    let turns = thread.pointer("/thread/turns").and_then(Value::as_array).cloned().unwrap_or_default();
    for turn in turns {
        let id = turn.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        events.push(TalkEvent::TurnStarted { turn: id.clone() });
        for item in turn.get("items").and_then(Value::as_array).into_iter().flatten() {
            events.extend(codex_item(item, true));
        }
        let status = turn.get("status").and_then(Value::as_str).unwrap_or("unknown").to_string();
        if status != "inProgress" && running != Some(id.as_str()) {
            events.push(TalkEvent::TurnEnded { turn: id, status });
        }
    }
    events
}

/// Who said words that went into a Codex thread. The World frames a resident's words with
/// `=== from <id> ===`; the user's own words go in as they were typed.
fn speaker(text: &str) -> (String, String) {
    if let Some(rest) = text.strip_prefix("=== from ")
        && let Some((from, words)) = rest.split_once(" ===\n")
    {
        return (from.to_string(), words.to_string());
    }
    ("user".to_string(), text.to_string())
}

/// The arguments of a tool call on one line: the command of a `run`, a path, or the JSON.
fn arguments_summary(arguments: &Value) -> String {
    for key in ["command", "file_path", "path", "input", "prompt", "to"] {
        if let Some(text) = arguments.get(key).and_then(Value::as_str) {
            return first_line(text);
        }
    }
    first_line(&arguments.to_string())
}

fn first_line(text: &str) -> String {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or_default().trim().to_string();
    if lines.next().is_some() { format!("{first} …") } else { first }
}
