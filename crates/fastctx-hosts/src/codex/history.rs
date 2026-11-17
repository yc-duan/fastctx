//! Completed history and its canonical ordering. Notification-only state is
//! retained by the World; it is not reconstructed from unrelated items.

use std::collections::HashMap;

use fastctx_types::session::{Happened, SessionEvent, TurnEnd, What};
use jiff::Timestamp;
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
        let mut events: Vec<SessionEvent> =
            active_voice.into_iter().map(|session| voice(seconds(metadata.get("createdAt")), session)).collect();
        events.extend(project_timeline(&timeline, &entries, running, owner, seconds(metadata.get("createdAt"))).events);
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

/// A voice session the timeline says was open where it begins.
pub(super) fn voice(at: Timestamp, session: String) -> SessionEvent {
    SessionEvent {
        at,
        happened: Happened::Changed {
            turn: None,
            change: fastctx_types::session::SessionChange::Voice {
                event: "active_session".into(),
                data: json!({"realtimeSessionId": session}),
            },
        },
    }
}

/// One turn of a projected timeline: where it begins in the thread's
/// timeline (when the timeline says), when it began, the latest time of its
/// events and of what follows it before the next turn, and whether it ended.
pub(super) struct TurnSpan {
    pub id: String,
    pub position: Option<u64>,
    pub start: Timestamp,
    pub last: Timestamp,
    pub ended: bool,
}

/// Session events projected from timeline entries and the turns they make.
pub(super) struct Projected {
    pub events: Vec<SessionEvent>,
    pub turns: Vec<TurnSpan>,
}

/// When an item began and was done, as its entry in `thread/items/list`
/// says: all of an entry a projection reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timing {
    pub started: Option<i64>,
    pub completed: Option<i64>,
}

impl Timing {
    pub fn of(entry: &Value) -> Self {
        Self { started: entry["startedAtMs"].as_i64(), completed: entry["completedAtMs"].as_i64() }
    }
}

/// The session events of `timeline` (entries in order), timed by the item
/// entries `entries`, beginning at `at`. A turn still running is not ended.
pub(super) fn project_timeline(
    timeline: &[Value],
    entries: &[Value],
    running: Option<&str>,
    owner: Option<&fastctx_types::Address>,
    at: Timestamp,
) -> Projected {
    // An item listed twice is timed by its last entry.
    let times: HashMap<(&str, &str), Timing> = entries
        .iter()
        .filter_map(|entry| Some(((entry["turnId"].as_str()?, entry["item"]["id"].as_str()?), Timing::of(entry))))
        .collect();
    let mut projector = Projector::new(running, owner, at);
    let mut events = Vec::new();
    for entry in timeline {
        projector.push(entry, &mut |turn, id| Ok(times.get(&(turn, id)).copied()), &mut events).expect("timed from memory");
    }
    Projected { events, turns: projector.finish() }
}

/// A projection of timeline entries handed to it one at a time, in order
/// (`project_timeline` hands them all; a whole reading kept on disk hands
/// them as it reads them back, design §28.51).
pub(super) struct Projector<'a> {
    running: Option<&'a str>,
    owner: Option<&'a fastctx_types::Address>,
    at: Timestamp,
    turns: Vec<TurnSpan>,
    /// Each turn's last span in `turns`.
    last: HashMap<String, usize>,
    messages: HashMap<String, String>,
    submissions: super::user_images::Submissions,
}

/// When the item `id` of a turn began and was done (`Timing`), where known.
pub(super) type Timed<'t> = dyn FnMut(&str, &str) -> Result<Option<Timing>, String> + 't;

impl<'a> Projector<'a> {
    pub(super) fn new(running: Option<&'a str>, owner: Option<&'a fastctx_types::Address>, at: Timestamp) -> Self {
        Self { running, owner, at, turns: Vec::new(), last: HashMap::new(), messages: HashMap::new(), submissions: Default::default() }
    }

    /// Projects `entry`, the next of the timeline, onto `events`.
    pub(super) fn push(&mut self, entry: &Value, timed: &mut Timed<'_>, events: &mut Vec<SessionEvent>) -> Result<(), String> {
        let turn = entry.get("turnId").and_then(Value::as_str).unwrap_or_default();
        let before = events.len();
        match entry.get("type").and_then(Value::as_str) {
            Some("turnStarted") => {
                self.at = seconds(entry.get("startedAt"));
                events.push(SessionEvent { at: self.at, happened: Happened::TurnStarted { turn: turn.into() } });
                self.turns.push(TurnSpan { id: turn.into(), position: entry["position"].as_u64(), start: self.at, last: self.at, ended: false });
                self.last.insert(turn.to_string(), self.turns.len() - 1);
            }
            Some("item") => {
                let item = &entry["item"];
                let listed = item.get("id").and_then(Value::as_str);
                let timing = match listed {
                    Some(id) => timed(turn, id)?,
                    None => None,
                };
                if let Some(completed) = timing.and_then(|timing| timing.completed) {
                    self.at = millis(Some(&Value::from(completed)));
                }
                let ordinal = self.submissions.observe(turn, item);
                let facts = items::agent_facts(item);
                if let Some(mut happened) = completed(turn, item, &mut self.messages) {
                    if let Happened::Heard { message: Some(message), .. } = &mut happened {
                        super::user_images::decorate(message, self.owner, turn, ordinal, false);
                    }
                    if let Happened::Finished { item, .. } = &mut happened {
                        // Timed by its own id, which is the listed one but
                        // where the projection named it otherwise.
                        let timing = if listed == Some(item.id.as_str()) { timing } else { timed(turn, &item.id)? };
                        if let Some(timing) = timing {
                            item.started_at = timing.started.and_then(|n| jiff::Timestamp::from_millisecond(n).ok());
                            if let Some(end) = timing.completed.and_then(|n| jiff::Timestamp::from_millisecond(n).ok()) {
                                item.thought_until(end);
                            }
                        }
                    }
                    events.push(SessionEvent { at: self.at, happened });
                }
                let at = self.at;
                events.extend(facts.into_iter().map(|change| SessionEvent { at, happened: Happened::Changed { turn: Some(turn.into()), change } }));
            }
            Some("realtime") => events.push(SessionEvent { at: self.at, happened: Happened::Realtime { item: entry["item"].clone(), complete: true } }),
            Some("turnCompleted") if self.running != Some(turn) => {
                self.at = seconds(entry.get("completedAt"));
                events.push(SessionEvent {
                    at: self.at,
                    happened: Happened::TurnEnded {
                        turn: turn.into(),
                        end: TurnEnd {
                            facts: None,
                            error: entry.get("error").filter(|e| !e.is_null()).cloned(),
                            how: turn_how(entry.get("status").and_then(Value::as_str)),
                            reason: entry.pointer("/error/message").and_then(Value::as_str).map(str::to_string),
                            duration_ms: entry.get("durationMs").and_then(Value::as_u64),
                            last_message: self.messages.remove(turn),
                        },
                    },
                });
                if let Some(at) = self.last.get(turn).copied() {
                    self.turns[at].ended = true;
                }
            }
            _ => {}
        }
        // An entry is of its own turn's span, a late item of an earlier turn
        // too (C1); one of no turn, of the turn it follows.
        let own = entry.get("turnId").and_then(Value::as_str).and_then(|turn| self.last.get(turn).copied());
        if let Some(span) = own.or(self.turns.len().checked_sub(1)).map(|at| &mut self.turns[at]) {
            for event in &events[before..] {
                span.last = span.last.max(event.at);
            }
        }
        Ok(())
    }

    /// The turns the projection made.
    pub(super) fn finish(self) -> Vec<TurnSpan> {
        self.turns
    }
}

pub(super) fn title(metadata: &Value) -> Vec<SessionEvent> {
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

pub(super) fn ordinary_timeline(turns: &[Value], entries: &[Value]) -> Vec<Value> {
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

pub(super) async fn pages_with_voice(
    server: &AppServer,
    method: &str,
    params: Value,
) -> Result<(Vec<Value>, Option<String>), Unanswered> {
    let mut entries = Vec::new();
    let active_voice = each_page(server, method, params, &mut |entry| {
        entries.push(entry);
        Ok(())
    })
    .await?;
    Ok((entries, active_voice))
}

/// Each entry of every page of `method`, handed to `each` as it comes rather
/// than gathered, and the voice session the last page says was open at its
/// start. `each` failing stops the listing, its reason said as `Refused`.
pub(super) async fn each_page(
    server: &AppServer,
    method: &str,
    mut params: Value,
    each: &mut (dyn FnMut(Value) -> Result<(), String> + Send),
) -> Result<Option<String>, Unanswered> {
    loop {
        let mut page = server.ask(method, params.clone()).await?;
        // Taken out of the page, not copied: a whole thread is read so (E4).
        let Some(Value::Array(data)) = page.get_mut("data").map(Value::take) else {
            return Err(Unanswered::Refused(format!("{method} answered without data")));
        };
        for entry in data {
            each(entry).map_err(Unanswered::Refused)?;
        }
        let active_voice = page.get("activeRealtimeSessionAtPageStart").and_then(Value::as_str).map(str::to_string);
        match page.get("nextCursor").and_then(Value::as_str) {
            Some(cursor) => params["cursor"] = json!(cursor),
            None => return Ok(active_voice),
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
