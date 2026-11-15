//! The projection as it was before it read its rows through a row source
//! (design §28.51 ⑤-3), word for word: what the projection must equal. Only
//! the tests use it.

use std::collections::{HashMap, HashSet};
use std::io;

use fastctx_types::session::{Happened, HostAgentStart, SessionChange, SessionEvent, What};
use jiff::Timestamp;
use serde_json::{Value, json};

use super::{Block, HistoryCut, Output, Row, Scanned, Transcript, synthetic_reply, valid_call, valid_result};
use super::super::subagents;

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

/// What the rows `scanned` holds show, the agents named in `agents` started
/// before them: their events, title, context and problems, as much of it as
/// `output` asks for.
pub(crate) fn project(
    scanned: Scanned,
    cuts: &[HistoryCut],
    earlier: &HashSet<String>,
    beside: subagents::Beside,
    agents: HashMap<String, HostAgentStart>,
    mut output: Output<'_>,
) -> io::Result<Transcript> {
    let Scanned { rows: super::Held { rows, by_uuid }, queued, titles, problems, problems_omitted, refused, .. } = scanned;
    let mut transcript = Transcript {
        refused,
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
        problems,
        problems_omitted,
        uuids: HashSet::new(),
        beside,
        agents,
    };
    transcript.uuids = by_uuid.keys().cloned().chain(queued.held).collect();
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
    // A compaction whose kept messages are missing from the file leaves the
    // context it holds unknown, not the conversation unreadable: what it
    // shows is read as ever, and what needs that context refuses on the
    // problem.
    let context_order = match order(&rows.iter().map(|row| &row.value).collect::<Vec<_>>()) {
        Ok(order) => order,
        Err(error) => {
            transcript.problems.push(format!("the context its last compaction left: {error}"));
            Vec::new()
        }
    };
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
        .is_some_and(|row| synthetic_reply(&row.value));

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
            .filter(|(_, row)| super::super::context::projected(&row.value))
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
            .all(|(&index, expected)| super::super::context::same(&super::super::context::projection(&rows[index].value), expected))
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
    // The context's messages only where they are asked for: a history and a
    // seam use none of them.
    if matches!(output, Output::Context) {
        transcript.context_uuids = context_order
            .iter()
            .map(|&index| &rows[index])
            .filter(|row| native_visible(row) && super::super::context::projected(&row.value))
            .filter_map(|row| row.value["uuid"].as_str().map(str::to_owned))
            .collect();
        transcript.context_times = context_order
            .iter()
            .map(|&index| &rows[index])
            .filter(|row| native_visible(row) && super::super::context::projected(&row.value))
            .filter_map(|row| row.value["timestamp"].as_str()?.parse().ok())
            .collect();
        transcript.context_messages = context_order
            .iter()
            .map(|&index| &rows[index])
            .filter(|row| native_visible(row) && super::super::context::projected(&row.value))
            .map(|row| super::super::context::projection(&row.value))
            .collect();
    }
    let visible = |row: &Row| {
        native_visible(row)
            && !row
                .value
                .get("uuid")
                .and_then(Value::as_str)
                .and_then(|id| by_uuid.get(id))
                .is_some_and(|index| cut_rows.contains(index))
    };
    // Claude Code names the summary a compaction begins with as the anchor
    // of what it kept (`compactMetadata`).
    let anchors: HashMap<&str, usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.compaction())
        .flat_map(|(index, row)| {
            let metadata = &row.value["compactMetadata"];
            [&metadata["preservedSegment"]["anchorUuid"], &metadata["preservedMessages"]["anchorUuid"]]
                .into_iter()
                .filter_map(Value::as_str)
                .map(move |uuid| (uuid, index))
        })
        .collect();
    let mut pairs: HashMap<&str, Pair> = HashMap::new();
    let mut summaries: HashMap<&str, Vec<usize>> = HashMap::new();
    for (row_index, row) in rows.iter().enumerate().filter(|(index, row)| times[*index].is_some() && visible(row)) {
        if row.message_problem().is_some() {
            continue;
        }
        if row.kind() == Some("user")
            && row.value["isCompactSummary"] == true
            && let Some(boundary) = summarized(&rows, &by_uuid, &anchors, row)
            && times[boundary].is_some()
            && visible(&rows[boundary])
            && let Some(uuid) = rows[boundary].value.get("uuid").and_then(Value::as_str)
        {
            summaries.entry(uuid).or_default().push(row_index);
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
        // What the rows before made, handed on (`Output`).
        output.hand(&mut transcript.events)?;
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
        if synthetic_reply(&row.value) {
            transcript.hidden(row, at);
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
                    // The user's words the World's plugin submitted, as they were said.
                    let words = match words {
                        Some(text) if row.value["origin"]["kind"] == "plugin" => Some(super::super::as_said(&text).to_owned()),
                        words => words,
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
    output.hand(&mut transcript.events)?;
    Ok(transcript)
}

/// The row of the compaction a summary row belongs to: the one whose record
/// names it as its anchor, else the first boundary above it along its
/// parents, past the attachments Claude Code writes between the two and
/// never across a message.
fn summarized(
    rows: &[Row],
    by_uuid: &HashMap<String, usize>,
    anchors: &HashMap<&str, usize>,
    row: &Row,
) -> Option<usize> {
    if let Some(&boundary) = row.value.get("uuid").and_then(Value::as_str).and_then(|uuid| anchors.get(uuid)) {
        return Some(boundary);
    }
    let mut parent = row.value.get("parentUuid").and_then(Value::as_str);
    for _ in 0..32 {
        let index = *by_uuid.get(parent?)?;
        let above = &rows[index];
        if above.compaction() {
            return Some(index);
        }
        if matches!(above.kind(), Some("user" | "assistant")) {
            return None;
        }
        parent = above.value.get("parentUuid").and_then(Value::as_str);
    }
    None
}

/// `super::super::context::order` as it was.
fn order(rows: &[&Value]) -> io::Result<Vec<usize>> {
    let boundary = rows.iter().rposition(|row| row["type"] == "system" && row["subtype"] == "compact_boundary");
    let mut order: Vec<_> = rows
        .iter()
        .enumerate()
        .skip(boundary.unwrap_or(0))
        .filter(|(_, row)| matches!(row["type"].as_str(), Some("user" | "assistant" | "attachment" | "system")))
        .map(|(index, _)| index)
        .collect();
    let Some(boundary) = boundary else { return Ok(order) };
    let by_uuid: HashMap<_, _> =
        rows.iter().enumerate().filter_map(|(index, row)| Some((row["uuid"].as_str()?, index))).collect();
    let metadata = &rows[boundary]["compactMetadata"];
    let (anchor, preserved) = if let Some(messages) = metadata.get("preservedMessages") {
        let anchor = messages["anchorUuid"]
            .as_str()
            .ok_or_else(|| io::Error::other("Claude preserved messages omit their anchor"))?;
        let preserved = messages["uuids"]
            .as_array()
            .ok_or_else(|| io::Error::other("Claude preserved messages omit their UUIDs"))?
            .iter()
            .map(|uuid| uuid.as_str().ok_or_else(|| io::Error::other("Claude preserved UUID is not a string")))
            .collect::<io::Result<Vec<_>>>()?;
        (anchor, preserved)
    } else if let Some(segment) = metadata.get("preservedSegment") {
        let anchor = segment["anchorUuid"]
            .as_str()
            .ok_or_else(|| io::Error::other("Claude preserved segment omits its anchor"))?;
        let head =
            segment["headUuid"].as_str().ok_or_else(|| io::Error::other("Claude preserved segment omits its head"))?;
        let mut tail =
            segment["tailUuid"].as_str().ok_or_else(|| io::Error::other("Claude preserved segment omits its tail"))?;
        let mut visited = HashSet::new();
        let mut preserved = Vec::new();
        loop {
            if !visited.insert(tail) {
                return Err(io::Error::other("Claude preserved segment has a UUID cycle"));
            }
            let index = *by_uuid
                .get(tail)
                .ok_or_else(|| io::Error::other("Claude preserved segment refers to an absent row"))?;
            preserved.push(tail);
            if tail == head {
                break;
            }
            tail = rows[index]["parentUuid"]
                .as_str()
                .ok_or_else(|| io::Error::other("Claude preserved segment does not reach its head"))?;
        }
        preserved.reverse();
        (anchor, preserved)
    } else {
        return Ok(order);
    };
    let mut unique = HashSet::new();
    let preserved: Vec<_> = preserved
        .into_iter()
        .map(|uuid| {
            if !unique.insert(uuid) {
                return Err(io::Error::other("Claude preserved messages repeat a UUID"));
            }
            by_uuid
                .get(uuid)
                .copied()
                .ok_or_else(|| io::Error::other("Claude preserved messages refer to an absent row"))
        })
        .collect::<io::Result<_>>()?;
    order.retain(|index| !preserved.contains(index));
    let anchor = order
        .iter()
        .position(|&index| rows[index]["uuid"].as_str() == Some(anchor))
        .ok_or_else(|| io::Error::other("Claude preserved anchor is absent from the current context"))?;
    order.splice(anchor + 1..anchor + 1, preserved);
    Ok(order)
}
