//! What a sentinel did, read as a conversation (`world-senses.md`, 看它做了什么):
//! each wake a turn, opened by why it woke, then its thinking and its words,
//! where it looked and what came back, the reflexes it set, and what it said
//! of what it watches. Read from what each wake leaves on its home's disk
//! (`runs/`); nothing is kept for the reading.

use std::path::Path;

use fastctx_agent::Run;
use fastctx_agent::model::{Message, ToolResult};
use fastctx_types::residents::{Notice, Woke, WorldFact};
use fastctx_types::sentinel::WakeReason;
use fastctx_types::sentinel_control::{Record, Wake};
use fastctx_types::session::{
    Content, Does, Happened, Item, SessionEvent, Speaker, Tool, ToolStatus, TurnEnd, TurnHow, What,
};
use jiff::Timestamp;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// How many of its latest wakes are read.
const WAKES: usize = 40;

/// The wakes recorded in the generation directory `directory`, unless they
/// are as `held` names them.
pub fn read(directory: &Path, held: Option<&str>) -> Result<Record, String> {
    if !directory.is_dir() {
        return Err("no such sentinel lives here for this account".into());
    }
    let runs = directory.join("runs");
    let mut files = match std::fs::read_dir(&runs) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let sequence = name.strip_suffix(".json")?.parse::<u64>().ok()?;
                Some((sequence, entry.path()))
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("reading {}: {error}", runs.display())),
    };
    files.sort();
    let earlier = files.len() > WAKES;
    let files = files.split_off(files.len().saturating_sub(WAKES));
    let mut stamp = Sha256::new();
    for (sequence, path) in &files {
        let meta = std::fs::metadata(path).map_err(|error| format!("reading {}: {error}", path.display()))?;
        let modified = meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok());
        stamp.update(format!("{sequence}:{}:{};", meta.len(), modified.map_or(0, |at| at.as_nanos())));
    }
    let version = format!("{:x}", stamp.finalize());
    if held == Some(version.as_str()) {
        return Ok(Record { version, unchanged: true, events: Vec::new(), earlier, summary: None, next: None });
    }
    let mut events = Vec::new();
    for (sequence, path) in files {
        // A wake being written is read again next time.
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(run) = serde_json::from_slice::<Run>(&bytes) else { continue };
        events.extend(wake(sequence, &run));
    }
    Ok(Record { version, unchanged: false, events, earlier, summary: None, next: None })
}

/// How many of its latest wakes the overview tells.
const OVERVIEW_WAKES: usize = 400;

/// When each wake recorded in the generation directory `directory` began
/// at or after `since` (milliseconds since the epoch), why, and what it
/// took, oldest first; and whether earlier wakes were left out. A wake
/// still being written is read next time.
pub fn wakes(directory: &Path, since: i64) -> Result<(Vec<Wake>, bool), String> {
    let runs = directory.join("runs");
    let mut files = match std::fs::read_dir(&runs) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let sequence = name.strip_suffix(".json")?.parse::<u64>().ok()?;
                Some((sequence, entry.path()))
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("reading {}: {error}", runs.display())),
    };
    files.sort();
    // A wake is written when it ends, so a file last changed before `since`
    // began before it.
    let cutoff = std::time::UNIX_EPOCH + std::time::Duration::from_millis(u64::try_from(since).unwrap_or(0));
    let mut earlier = false;
    files.retain(|(_, path)| {
        let recent = std::fs::metadata(path).and_then(|meta| meta.modified()).map_or(true, |at| at >= cutoff);
        earlier |= !recent;
        recent
    });
    earlier |= files.len() > OVERVIEW_WAKES;
    let files = files.split_off(files.len().saturating_sub(OVERVIEW_WAKES));
    let mut wakes = Vec::new();
    for (_, path) in files {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(run) = serde_json::from_slice::<Run>(&bytes) else { continue };
        let input = match run.messages.first() {
            Some(Message::User { text }) => serde_json::from_str::<Value>(text).unwrap_or(Value::Null),
            _ => Value::Null,
        };
        let Some(at) = input.get("now").and_then(Value::as_i64) else { continue };
        if at < since {
            earlier = true;
            continue;
        }
        wakes.push(Wake {
            at,
            why: why(&input).0,
            input: run.usage.input,
            output: run.usage.output,
            cached: run.usage.cache_read,
            requests: run.requests,
            elapsed_ms: run.elapsed_ms,
            failed: run.error.is_some(),
        });
    }
    Ok((wakes, earlier))
}

/// One wake, as a turn.
fn wake(sequence: u64, run: &Run) -> Vec<SessionEvent> {
    let turn = format!("w{sequence}");
    let input = match run.messages.first() {
        Some(Message::User { text }) => serde_json::from_str::<Value>(text).unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let began = input
        .get("now")
        .and_then(Value::as_i64)
        .and_then(|now| Timestamp::from_millisecond(now).ok())
        .unwrap_or_default();
    let at = |happened: Happened| SessionEvent { at: began, happened };
    let (why, seen) = why(&input);
    let mut events = vec![
        at(Happened::TurnStarted { turn: turn.clone() }),
        at(Happened::Heard {
            delivery: None,
            turn: Some(turn.clone()),
            from: Some(Speaker::World),
            text: seen.clone(),
            message: None,
            notice: Some(Box::new(Notice::World { fact: WorldFact::Woke { why }, text: seen })),
        }),
    ];
    let mut index = 0;
    let mut item = |what: What, detail: Option<Value>| {
        index += 1;
        at(Happened::Finished {
            turn: turn.clone(),
            item: Item { id: format!("{turn}-{index}"), draft: None, started_at: None, detail, what },
        })
    };
    for (position, message) in run.messages.iter().enumerate() {
        let Message::Assistant { text, calls, raw } = message else { continue };
        for thought in thinking(raw) {
            events.push(item(What::Thinking { text: thought, duration_ms: None }, None));
        }
        if !text.trim().is_empty() {
            events.push(item(What::Message { text: text.clone() }, None));
        }
        let results: &[ToolResult] = match run.messages.get(position + 1) {
            Some(Message::Results { results }) => results,
            _ => &[],
        };
        for call in calls {
            let result = results.iter().find(|result| result.id == call.id);
            let arguments = &call.arguments;
            let said = |field: &str| arguments.get(field).and_then(Value::as_str).map(str::to_string);
            // What it said of what it watches is its words, not a tool.
            if call.name == "report" {
                let words = said("reason").or_else(|| said("thinks")).unwrap_or_default();
                if !words.trim().is_empty() {
                    events.push(item(What::Message { text: words }, Some(serde_json::json!({ "report": arguments }))));
                }
                continue;
            }
            let (name, machine, does) = match call.name.as_str() {
                "look" => (
                    "look",
                    said("machine").and_then(|machine| machine.parse().ok()),
                    Does::Command { command: said("command").unwrap_or_default(), cwd: None, exit: None, job: None },
                ),
                "search" => ("search", None, Does::Search { query: said("query").unwrap_or_default() }),
                "fetch_web" => ("WebFetch", None, Does::Call),
                other => (other, None, Does::Call),
            };
            let status = match result {
                Some(result) if result.error => ToolStatus::Failed,
                Some(_) => ToolStatus::Done,
                None if run.ended => ToolStatus::Cut,
                None => ToolStatus::Running,
            };
            let output = result.map(|result| vec![Content::Text { text: result.content.clone() }]).unwrap_or_default();
            let tool = Tool {
                run: None,
                name: name.to_string(),
                server: None,
                machine,
                does,
                input: arguments.clone(),
                output,
                status,
                duration_ms: None,
            };
            events.push(item(What::Tool(Box::new(tool)), None));
        }
    }
    if run.ended {
        let ended = began.checked_add(jiff::SignedDuration::from_millis(run.elapsed_ms as i64)).unwrap_or(began);
        let how = if run.error.is_some() { TurnHow::Failed } else { TurnHow::Completed };
        events.push(SessionEvent {
            at: ended,
            happened: Happened::TurnEnded {
                turn,
                end: TurnEnd {
                    how,
                    reason: run.error.clone(),
                    error: None,
                    duration_ms: Some(run.elapsed_ms),
                    facts: None,
                    last_message: None,
                },
            },
        });
    }
    events
}

/// Why it woke, and what the reflex that woke it saw.
fn why(input: &Value) -> (Woke, String) {
    let Some(wake) = input.get("wake") else { return (Woke::Review, String::new()) };
    match serde_json::from_value::<WakeReason>(wake.clone()) {
        Ok(WakeReason::Initial) => (Woke::Started, String::new()),
        Ok(WakeReason::Time { .. }) => (Woke::Due, String::new()),
        Ok(WakeReason::Reflex { observation, .. }) => {
            (Woke::Reflex { source: observation.source.clone() }, observation.text.clone())
        }
        Ok(WakeReason::Broken { reason, .. }) => (Woke::Broken { reason: reason.clone() }, reason),
        Ok(WakeReason::Withdrawn) => (Woke::Withdrawn, String::new()),
        Ok(WakeReason::Goal { .. }) | Err(_) => (Woke::Review, String::new()),
    }
}

/// The thinking a model's reply carries: Anthropic's thinking blocks, or the
/// reasoning an OpenAI-compatible service puts beside the message.
fn thinking(raw: &Value) -> Vec<String> {
    let mut thoughts = Vec::new();
    if let Some(blocks) = raw.as_array() {
        for block in blocks {
            if block.get("type").and_then(Value::as_str) == Some("thinking")
                && let Some(text) = block.get("thinking").and_then(Value::as_str)
                && !text.trim().is_empty()
            {
                thoughts.push(text.to_string());
            }
        }
    }
    for field in ["reasoning_content", "reasoning"] {
        if let Some(text) = raw.get(field).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            thoughts.push(text.to_string());
        }
    }
    thoughts
}
