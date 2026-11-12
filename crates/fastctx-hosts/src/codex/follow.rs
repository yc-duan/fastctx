//! A Codex thread read past where it was read through (sync-v2-impl-design
//! §10, §21). Where it was read through is a turn: the next reading lists
//! the thread's timeline from the newest entry back to that turn's start,
//! and the items of each turn from there, and projects them as a whole
//! reading does. The seam then moves to the latest turn that every turn and
//! event before it is settled for: each turn before it ended, every event
//! before it earlier than its start, and every turn the World saw begin
//! before it ended by then.
//!
//! What lies before the seam is not taken on trust: each reading lists the
//! items of one settled turn again, in turn, and a turn whose items are not
//! as they were — or a seam's turn no longer where it was — reads the thread
//! whole. Codex promises neither the cursor nor that a turn's summary tells
//! every change of its items, so neither is kept or relied on.

use super::history::{self, TurnSpan};
use super::server::{AppServer, Unanswered};
use fastctx_types::session::SessionEvent;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Entries asked for at a time, reading back from the newest.
const PAGE: u32 = 128;
/// How many times a thread that changed while it was read is read again
/// before it is left for a later reading.
const STEADY_TRIES: u32 = 4;

/// Where a thread was read through.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Seam {
    /// The turn the next reading begins with; none before one was read.
    pub turn: Option<String>,
    /// Where that turn begins in the thread's timeline, when it says.
    pub position: Option<u64>,
    /// The latest time of an event before that turn.
    pub before: Option<Timestamp>,
    /// The turns before it, oldest first: each one's id and the digest of
    /// its items as they were listed.
    pub settled: Vec<(String, u64)>,
    /// The settled turn listed again at the next reading.
    pub rotation: usize,
}

/// Why a thread is read whole after all.
#[derive(Debug)]
pub enum Restart {
    /// Its history is not kept in pages: read whole as ever.
    Legacy,
    /// The seam's turn is not where it was.
    Moved,
    /// A settled turn's items are not as they were.
    Changed(String),
}

impl std::fmt::Display for Restart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Restart::Legacy => write!(f, "its history is not kept in pages"),
            Restart::Moved => write!(f, "the turn it was read through is not where it was"),
            Restart::Changed(turn) => write!(f, "the items of turn {turn} are not as they were read"),
        }
    }
}

/// What a reading past the seam found.
pub struct Tail {
    /// The events of the turns from the seam on, with the thread's title.
    pub events: Vec<SessionEvent>,
    /// Read from its start: the events are the whole history.
    pub whole: bool,
    /// When the seam's turn began: events from then on are the window's.
    pub since: Option<Timestamp>,
    /// Where the thread is read through now.
    pub seam: Seam,
    /// When the turn the seam moved to began: before then, events are
    /// settled from now on.
    pub settled: Option<Timestamp>,
}

/// Reads `thread` past `seam` (from its start when the seam has no turn).
/// `accept` says whether every turn the World saw begin before a time had
/// ended by then.
pub(super) async fn tail(
    server: &AppServer,
    thread: &str,
    running: Option<&str>,
    owner: Option<&fastctx_types::Address>,
    seam: &Seam,
    accept: &(dyn Fn(Timestamp) -> bool + Send + Sync),
) -> Result<Result<Tail, Restart>, String> {
    // Read again until two readings of what the thread is like now agree
    // around the reading of its entries (C3): the RPCs share no snapshot.
    for _ in 0..STEADY_TRIES {
        let before = shape(server, thread).await?;
        let read = once(server, thread, running, owner, seam, accept).await?;
        if shape(server, thread).await? == before {
            return Ok(read);
        }
    }
    // Never what two readings did not agree on (N3): read again later.
    Err(format!("thread {thread} changed each of the {STEADY_TRIES} times it was read"))
}

/// What tells whether a thread changed between two moments: when Codex last
/// changed it, and where its timeline ends.
async fn shape(server: &AppServer, thread: &str) -> Result<(Value, Option<u64>), String> {
    let answer = server.request("thread/read", json!({"threadId": thread, "includeTurns": false})).await?;
    let updated = answer["thread"]["updatedAt"].clone();
    let end = match server.ask("thread/timeline/list", json!({"threadId": thread, "limit": 1})).await {
        Ok(page) => page["data"].as_array().and_then(|data| data.iter().filter_map(|entry| entry["position"].as_u64()).max()),
        Err(_) => None,
    };
    Ok((updated, end))
}

/// One reading past `seam`, as `tail` describes.
async fn once(
    server: &AppServer,
    thread: &str,
    running: Option<&str>,
    owner: Option<&fastctx_types::Address>,
    seam: &Seam,
    accept: &(dyn Fn(Timestamp) -> bool + Send + Sync),
) -> Result<Result<Tail, Restart>, String> {
    let answer = server.request("thread/read", json!({"threadId": thread, "includeTurns": false})).await?;
    let metadata = &answer["thread"];
    if metadata.get("historyMode").and_then(Value::as_str) != Some("paginated") {
        return Ok(Err(Restart::Legacy));
    }
    let whole = seam.turn.is_none();
    let read = match &seam.turn {
        None => whole_entries(server, thread).await?,
        Some(turn) => match window_entries(server, thread, turn, seam.position).await? {
            Some(read) => read,
            None => return Ok(Err(Restart::Moved)),
        },
    };
    let Some(Read { timeline, items, voice }) = read else {
        // Not begun yet: nothing but its title.
        return Ok(Ok(Tail { events: history::title(metadata), whole, since: None, seam: Seam::default(), settled: None }));
    };
    // One settled turn listed again, in turn.
    let mut rotation = seam.rotation;
    if !whole && !seam.settled.is_empty() {
        let at = rotation % seam.settled.len();
        let (turn, digest) = &seam.settled[at];
        let listed = turn_items(server, thread, turn).await?;
        if self::digest(&listed) != *digest {
            return Ok(Err(Restart::Changed(turn.clone())));
        }
        rotation = at + 1;
    }
    let created = super::seconds(metadata.get("createdAt"));
    let projected = history::project_timeline(&timeline, &items, running, owner, created);
    // As a whole reading has it: a voice session open at the thread's start.
    let mut events = voice.into_iter().map(|session| history::voice(created, session)).collect::<Vec<_>>();
    events.extend(projected.events);
    events.extend(history::title(metadata));
    let digests = digests(&items);
    let (next, settled) = advance(seam, &projected.turns, &digests, running, accept, whole);
    let since = if whole { None } else { projected.turns.first().map(|span| span.start) };
    Ok(Ok(Tail { events, whole, since, seam: Seam { rotation, ..next }, settled }))
}

/// Timeline entries in order, the item entries of their turns, and the
/// voice session a whole timeline says was open at its start.
struct Read {
    timeline: Vec<Value>,
    items: Vec<Value>,
    voice: Option<String>,
}

/// The whole timeline; none before the thread's first words.
async fn whole_entries(server: &AppServer, thread: &str) -> Result<Option<Read>, String> {
    let turns = match history::pages(server, "thread/turns/list", json!({"threadId":thread,"sortDirection":"asc","itemsView":"notLoaded"})).await {
        Ok(turns) => turns,
        Err(Unanswered::Rpc { code: -32600, message, data })
            if data.is_null()
                && message == format!("thread {thread} is not materialized yet; thread/turns/list is unavailable before first user message") =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };
    let items = history::pages(server, "thread/items/list", json!({"threadId": thread, "sortDirection": "asc"}))
        .await
        .map_err(|error| error.to_string())?;
    let (mut timeline, voice) = match history::pages_with_voice(server, "thread/timeline/list", json!({"threadId": thread})).await {
        Ok(timeline) => timeline,
        Err(Unanswered::Rpc { code: -32600 | -32601, .. }) => (history::ordinary_timeline(&turns, &items), None),
        Err(error) => return Err(error.to_string()),
    };
    timeline.sort_by_key(|entry| entry["position"].as_u64());
    Ok(Some(Read { timeline, items, voice }))
}

/// The timeline from the start of `turn` on, read back from the newest
/// entry, and the items of its turns; none when the turn does not begin
/// where it did.
async fn window_entries(server: &AppServer, thread: &str, turn: &str, position: Option<u64>) -> Result<Option<Option<Read>>, String> {
    let mut timeline = Vec::new();
    let mut params = json!({"threadId": thread, "limit": PAGE});
    let found = loop {
        let page = match server.ask("thread/timeline/list", params.clone()).await {
            Ok(page) => page,
            Err(Unanswered::Rpc { code: -32600 | -32601, .. }) => return ordinary_window(server, thread, turn).await,
            Err(error) => return Err(error.to_string()),
        };
        let data = page.get("data").and_then(Value::as_array).ok_or("thread/timeline/list answered without data")?;
        let begins = data.iter().find(|entry| entry["type"] == "turnStarted" && entry["turnId"] == turn).and_then(|entry| entry["position"].as_u64());
        timeline.extend(data.iter().cloned());
        if let Some(begins) = begins {
            break Some(begins);
        }
        match page.get("nextCursor").and_then(Value::as_str) {
            Some(cursor) => params["cursor"] = json!(cursor),
            None => break None,
        }
    };
    let Some(begins) = found else { return Ok(None) };
    if position.is_some_and(|position| position != begins) {
        return Ok(None);
    }
    timeline.retain(|entry| entry["position"].as_u64().is_some_and(|at| at >= begins));
    timeline.sort_by_key(|entry| entry["position"].as_u64());
    // The items of each turn the window names, an earlier turn's late item
    // among them, listed by turn: the thread's items are not kept a turn
    // at a time in their order (C1).
    let mut items = Vec::new();
    for turn in turns_of(&timeline) {
        items.extend(turn_items(server, thread, &turn).await?);
    }
    Ok(Some(Some(Read { timeline, items, voice: None })))
}

/// The window as the turns and items say, for a Codex without the timeline
/// (or one refusing it for this thread): the turns from `turn` on, newest
/// first, and each one's items.
async fn ordinary_window(server: &AppServer, thread: &str, turn: &str) -> Result<Option<Option<Read>>, String> {
    let mut turns = Vec::new();
    let mut params = json!({"threadId": thread, "sortDirection": "desc", "itemsView": "notLoaded", "limit": PAGE});
    loop {
        let page = server.ask("thread/turns/list", params.clone()).await.map_err(|error| error.to_string())?;
        let data = page.get("data").and_then(Value::as_array).ok_or("thread/turns/list answered without data")?;
        let at = data.iter().position(|found| found["id"] == turn);
        match at {
            Some(at) => {
                turns.extend(data[..=at].iter().cloned());
                break;
            }
            None => turns.extend(data.iter().cloned()),
        }
        match page.get("nextCursor").and_then(Value::as_str) {
            Some(cursor) => params["cursor"] = json!(cursor),
            None => return Ok(None),
        }
    }
    turns.reverse();
    let mut items = Vec::new();
    for found in &turns {
        if let Some(id) = found["id"].as_str() {
            items.extend(turn_items(server, thread, id).await?);
        }
    }
    let mut timeline = history::ordinary_timeline(&turns, &items);
    // Its places are the window's own, not the thread's.
    for entry in &mut timeline {
        entry["position"] = Value::Null;
    }
    Ok(Some(Some(Read { timeline, items, voice: None })))
}

/// The turns `timeline`'s entries are of, each once, in order of first
/// mention.
fn turns_of(timeline: &[Value]) -> Vec<String> {
    let mut turns: Vec<String> = Vec::new();
    for turn in timeline.iter().filter_map(|entry| entry["turnId"].as_str()) {
        if !turns.iter().any(|known| known == turn) {
            turns.push(turn.to_string());
        }
    }
    turns
}

/// Every item entry of `turn`, in order.
async fn turn_items(server: &AppServer, thread: &str, turn: &str) -> Result<Vec<Value>, String> {
    history::pages(server, "thread/items/list", json!({"threadId": thread, "turnId": turn, "sortDirection": "asc"}))
        .await
        .map_err(|error| error.to_string())
}

/// The digest of a turn's item entries as listed.
fn digest(entries: &[Value]) -> u64 {
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    for entry in entries {
        hasher.update(&serde_json::to_vec(entry).unwrap_or_default());
        hasher.update(b"\n");
    }
    hasher.digest()
}

/// Each turn's digest of the item entries `items` list.
fn digests(items: &[Value]) -> BTreeMap<String, u64> {
    let mut by_turn: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for entry in items {
        if let Some(turn) = entry["turnId"].as_str() {
            by_turn.entry(turn.to_string()).or_default().push(entry.clone());
        }
    }
    by_turn.into_iter().map(|(turn, entries)| (turn, digest(&entries))).collect()
}

/// Where the seam goes past `seam`, given the turns read from it on: the
/// latest turn every turn before which ended, every event before which is
/// earlier than its start, and before whose start every turn the World saw
/// begin had ended. With it, when it began.
fn advance(
    seam: &Seam,
    turns: &[TurnSpan],
    digests: &BTreeMap<String, u64>,
    running: Option<&str>,
    accept: &(dyn Fn(Timestamp) -> bool + Send + Sync),
    whole: bool,
) -> (Seam, Option<Timestamp>) {
    if turns.is_empty() {
        let kept = if whole { Seam::default() } else { seam.clone() };
        return (kept, None);
    }
    let base = if whole { Seam::default() } else { seam.clone() };
    let mut best = 0;
    let mut before = base.before;
    let mut ended = true;
    let mut latest = vec![before];
    for (index, span) in turns.iter().enumerate() {
        if index > 0 && ended && before.is_none_or(|before| span.start > before) && accept(span.start) {
            best = index;
        }
        before = Some(before.map_or(span.last, |before| before.max(span.last)));
        latest.push(before);
        ended &= span.ended && running != Some(span.id.as_str());
    }
    let mut settled = base.settled;
    for span in &turns[..best] {
        settled.push((span.id.clone(), digests.get(&span.id).copied().unwrap_or_else(|| digest(&[]))));
    }
    let chosen = &turns[best];
    (Seam { turn: Some(chosen.id.clone()), position: chosen.position, before: latest[best], settled, rotation: base.rotation }, Some(chosen.start))
}
