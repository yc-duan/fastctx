//! Completed history and its canonical ordering. Notification-only state is
//! retained by the World; it is not reconstructed from unrelated items.

use std::collections::HashMap;

use fastctx_types::session::{Happened, SessionEvent, TurnEnd, What};
use serde_json::{Value, json};

use super::{
    items, millis, seconds,
    server::{AppServer, Unanswered},
    turn_how,
};

pub(super) async fn read(
    server: &AppServer,
    thread: &str,
    running: Option<&str>,
    owner: Option<&fastctx_types::Address>,
) -> Result<Vec<SessionEvent>, String> {
    let answer = server.request("thread/read", json!({"threadId": thread, "includeTurns": false})).await?;
    let metadata = &answer["thread"];
    let mut events = if metadata.get("historyMode").and_then(Value::as_str) == Some("paginated") {
        let turns = match pages(
            server,
            "thread/turns/list",
            json!({"threadId":thread,"sortDirection":"asc","itemsView":"notLoaded"}),
        )
        .await
        {
            Ok(turns) => turns,
            Err(Unanswered::Rpc { code: -32600, message, data })
                if data.is_null()
                    && message
                        == format!(
                            "thread {thread} is not materialized yet; thread/turns/list is unavailable before first user message"
                        ) =>
            {
                return Ok(title(metadata));
            }
            Err(error) => return Err(error.to_string()),
        };
        let entries = pages(server, "thread/items/list", json!({"threadId": thread, "sortDirection": "asc"}))
            .await
            .map_err(|e| e.to_string())?;
        let times: HashMap<(&str, &str), &Value> = entries
            .iter()
            .filter_map(|entry| Some(((entry["turnId"].as_str()?, entry["item"]["id"].as_str()?), entry)))
            .collect();
        let (mut timeline, active_voice) =
            match pages_with_voice(server, "thread/timeline/list", json!({"threadId": thread})).await {
                Ok(timeline) => timeline,
                // The 0.160 timeline RPC excludes archived threads, and a
                // Codex may refuse it otherwise or not have it at all; its
                // ordinary turns and items give the whole history but voice.
                Err(Unanswered::Rpc { code: -32600 | -32601, .. }) => (ordinary_timeline(&turns, &entries), None),
                Err(error) => return Err(error.to_string()),
            };
        // Timeline pages run newest to oldest, with each page ordered forward.
        timeline.sort_by_key(|entry| entry["position"].as_u64());
        let mut events: Vec<SessionEvent> = active_voice
            .into_iter()
            .map(|session| SessionEvent {
                at: seconds(metadata.get("createdAt")),
                happened: Happened::Changed {
                    turn: None,
                    change: fastctx_types::session::SessionChange::Voice {
                        event: "active_session".into(),
                        data: json!({"realtimeSessionId": session}),
                    },
                },
            })
            .collect();
        let mut messages: HashMap<String, String> = HashMap::new();
        let mut submissions = super::user_images::Submissions::default();
        let mut at = seconds(metadata.get("createdAt"));
        for entry in timeline {
            let turn = entry.get("turnId").and_then(Value::as_str).unwrap_or_default();
            match entry.get("type").and_then(Value::as_str) {
                Some("turnStarted") => {
                    at = seconds(entry.get("startedAt"));
                    events.push(SessionEvent { at, happened: Happened::TurnStarted { turn: turn.into() } });
                }
                Some("item") => {
                    let item = &entry["item"];
                    if let Some(timing) = item.get("id").and_then(Value::as_str).and_then(|id| times.get(&(turn, id)))
                        && timing["completedAtMs"].is_i64()
                    {
                        at = millis(timing.get("completedAtMs"));
                    }
                    let ordinal = submissions.observe(turn, item);
                    let facts = items::agent_facts(item);
                    if let Some(mut happened) = completed(turn, item, &mut messages) {
                        if let Happened::Heard { message: Some(message), .. } = &mut happened {
                            super::user_images::decorate(message, owner, turn, ordinal, false);
                        }
                        if let Happened::Finished { item, .. } = &mut happened
                            && let Some(timing) = times.get(&(turn, item.id.as_str()))
                        {
                            item.started_at =
                                timing["startedAtMs"].as_i64().and_then(|n| jiff::Timestamp::from_millisecond(n).ok());
                            if let Some(end) =
                                timing["completedAtMs"].as_i64().and_then(|n| jiff::Timestamp::from_millisecond(n).ok())
                            {
                                item.thought_until(end);
                            }
                        }
                        events.push(SessionEvent { at, happened });
                    }
                    events.extend(facts.into_iter().map(|change| SessionEvent {
                        at,
                        happened: Happened::Changed { turn: Some(turn.into()), change },
                    }));
                }
                Some("realtime") => events.push(SessionEvent {
                    at,
                    happened: Happened::Realtime { item: entry["item"].clone(), complete: true },
                }),
                Some("turnCompleted") if running != Some(turn) => {
                    at = seconds(entry.get("completedAt"));
                    events.push(SessionEvent {
                        at,
                        happened: Happened::TurnEnded {
                            turn: turn.into(),
                            end: TurnEnd {
                                facts: None,
                                error: entry.get("error").filter(|e| !e.is_null()).cloned(),
                                how: turn_how(entry.get("status").and_then(Value::as_str)),
                                reason: entry.pointer("/error/message").and_then(Value::as_str).map(str::to_string),
                                duration_ms: entry.get("durationMs").and_then(Value::as_u64),
                                last_message: messages.remove(turn),
                            },
                        },
                    });
                }
                _ => {}
            }
        }
        events
    } else {
        match server.ask("thread/read", json!({"threadId": thread, "includeTurns": true})).await {
            Ok(answer) => project(&answer, running, owner),
            Err(Unanswered::Rpc { code: -32600, message, data })
                if data.is_null()
                    && message
                        == format!(
                            "thread {thread} is not materialized yet; includeTurns is unavailable before first user message"
                        ) =>
            {
                Vec::new()
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    events.extend(title(metadata));
    Ok(events)
}

fn title(metadata: &Value) -> Vec<SessionEvent> {
    let mut events = vec![SessionEvent {
        at: seconds(metadata.get("updatedAt")),
        happened: Happened::Changed {
            turn: None,
            change: fastctx_types::session::SessionChange::Metadata { metadata: metadata.clone() },
        },
    }];
    events.extend(metadata.get("name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(|title| {
        SessionEvent { at: seconds(metadata.get("updatedAt")), happened: Happened::Titled { title: title.into() } }
    }));
    events
}

fn ordinary_timeline(turns: &[Value], entries: &[Value]) -> Vec<Value> {
    let mut timeline = Vec::new();
    for turn in turns {
        timeline.push(json!({"type":"turnStarted","turnId":turn["id"],"startedAt":turn["startedAt"]}));
        timeline.extend(
            entries
                .iter()
                .filter(|e| e["turnId"] == turn["id"])
                .map(|entry| json!({"type":"item","turnId":turn["id"],"item":entry["item"]})),
        );
        if turn["status"] != "inProgress" {
            let mut end = turn.clone();
            end["type"] = json!("turnCompleted");
            end["turnId"] = turn["id"].clone();
            timeline.push(end);
        }
    }
    for (position, entry) in timeline.iter_mut().enumerate() {
        entry["position"] = json!(position);
    }
    timeline
}

pub(super) async fn pages(server: &AppServer, method: &str, params: Value) -> Result<Vec<Value>, Unanswered> {
    pages_with_voice(server, method, params).await.map(|(entries, _)| entries)
}

async fn pages_with_voice(
    server: &AppServer,
    method: &str,
    mut params: Value,
) -> Result<(Vec<Value>, Option<String>), Unanswered> {
    let mut entries = Vec::new();
    loop {
        let page = server.ask(method, params.clone()).await?;
        let data = page
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| Unanswered::Refused(format!("{method} answered without data")))?;
        entries.extend(data.iter().cloned());
        let active_voice = page.get("activeRealtimeSessionAtPageStart").and_then(Value::as_str).map(str::to_string);
        match page.get("nextCursor").and_then(Value::as_str) {
            Some(cursor) => params["cursor"] = json!(cursor),
            None => return Ok((entries, active_voice)),
        }
    }
}

fn completed(turn: &str, raw: &Value, messages: &mut HashMap<String, String>) -> Option<Happened> {
    match items::read(raw, true) {
        items::Read::Words { text, message } => Some(Happened::Heard {
            delivery: None,
            turn: Some(turn.into()),
            from: None,
            text,
            message: Some(Box::new(message)),
            notice: None,
        }),
        items::Read::Item(item) => {
            if let What::Message { text } = &item.what {
                messages.insert(turn.into(), text.clone());
            }
            Some(Happened::Finished { turn: turn.into(), item })
        }
    }
}
/// A thread's history from `thread/read`, as session events.
pub(super) fn project(
    answer: &Value,
    running: Option<&str>,
    owner: Option<&fastctx_types::Address>,
) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let mut submissions = super::user_images::Submissions::default();
    for turn in answer.pointer("/thread/turns").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = turn.get("id").and_then(Value::as_str) else { continue };
        let started = seconds(turn.get("startedAt"));
        let mut push = |at, happened| events.push(SessionEvent { at, happened });
        push(started, Happened::TurnStarted { turn: id.to_string() });
        let mut last_message = None;
        for item in turn.get("items").and_then(Value::as_array).into_iter().flatten() {
            let turn = id.to_string();
            let ordinal = submissions.observe(&turn, item);
            match items::read(item, true) {
                items::Read::Words { text, mut message } => {
                    super::user_images::decorate(&mut message, owner, &turn, ordinal, true);
                    push(
                        started,
                        Happened::Heard {
                            delivery: None,
                            turn: Some(turn),
                            from: None,
                            text,
                            message: Some(Box::new(message)),
                            notice: None,
                        },
                    )
                }
                items::Read::Item(found) => {
                    if let What::Message { text } = &found.what {
                        last_message = Some(text.clone());
                    }
                    push(started, Happened::Finished { turn: turn.clone(), item: found });
                    for change in items::agent_facts(item) {
                        push(started, Happened::Changed { turn: Some(turn.clone()), change });
                    }
                }
            }
        }
        let status = turn.get("status").and_then(Value::as_str);
        // The record says a turn that is still running was interrupted
        // until it has been written to the end.
        if status == Some("inProgress") || running == Some(id) {
            continue;
        }
        let end = TurnEnd {
            facts: None,
            error: turn.get("error").filter(|e| !e.is_null()).cloned(),
            how: turn_how(status),
            reason: turn.pointer("/error/message").and_then(Value::as_str).map(str::to_string),
            duration_ms: turn.get("durationMs").and_then(Value::as_u64),
            last_message,
        };
        push(seconds(turn.get("completedAt")), Happened::TurnEnded { turn: id.to_string(), end });
    }
    events
}
