//! A Codex thread read past where it was read through (sync-v2-impl-design
//! §10, §21). Where it was read through is a turn: the next reading lists
//! the thread's timeline from the newest entry back to that turn's start,
//! and the items of each turn from there, and projects them as a whole
//! reading does. The seam then moves to the latest turn that every turn and
//! event before it is settled for: each turn before it ended, every event
//! before it earlier than its start, and every turn the World saw begin
//! before it ended by then.
//!
//! What lies before the seam is not taken on trust: each reading lists one
//! page of the items of a settled turn again, the settled turns in turn, and
//! a turn whose items are not as they were once its last page is in — or a
//! seam's turn no longer where it was — reads the thread whole. A page at a
//! time keeps a reading's cost that of the turn under way, not of the
//! largest turn settled (E3); the background check reads the thread whole
//! within the hour as ever (§10). Codex promises neither the cursor nor that
//! a turn's summary tells every change of its items: a cursor kept for the
//! next page that Codex no longer takes lists that turn from its first page
//! again, and nothing is concluded from a summary.

pub use super::history::Timing;
use super::history::{self, TurnSpan};
use super::server::{AppServer, Unanswered};
use fastctx_types::session::SessionEvent;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Entries asked for at a time, reading back from the newest.
const PAGE: u32 = 128;
/// Item entries asked for at a time: the most Codex gives (0.160), four
/// times what it gives unasked — a long turn's items are listed in a
/// quarter of the exchanges.
const ITEMS_PAGE: u32 = 100;
/// Entries taken back at a time from where a whole reading is kept.
const KEPT_AT_ONCE: usize = 256;

/// Where a whole reading of a thread is kept as it is read, rather than held
/// whole in memory (design §28.51): when each item listed began and was
/// done, the timeline's entries as they are listed, and the session events
/// made of them, which the reading hands on here instead of returning them.
/// The reading is the same as one held in memory makes, entry for entry and
/// event for event.
pub trait Keeping: Send {
    /// A reading begins, or begins again: whatever was kept before is let go.
    fn begin(&mut self) -> Result<(), String>;
    /// When the item `id` of `turn` began and was done, as listed; the last
    /// listing of an item stands.
    fn time(&mut self, turn: &str, id: &str, timing: Timing) -> Result<(), String>;
    /// When the item `id` of `turn` began and was done, if it was listed.
    fn timing(&mut self, turn: &str, id: &str) -> Result<Option<Timing>, String>;
    /// An item entry kept whole, in the order listed: for a thread whose
    /// timeline Codex will not list, made of its turns and items instead.
    /// `turn` is the entry's turn as JSON text.
    fn item(&mut self, turn: &str, entry: Value) -> Result<(), String>;
    /// The item entries kept whole of the turn `turn` (as JSON text), in the
    /// order listed, from the `from`th on, at most `limit`.
    fn items_of(&mut self, turn: &str, from: usize, limit: usize) -> Result<Vec<Value>, String>;
    /// The next timeline entry, as listed.
    fn entry(&mut self, entry: Value) -> Result<(), String>;
    /// The timeline entries kept, ordered by their position as a stable sort
    /// orders them (one without a position first), from the `from`th on, at
    /// most `limit`: called with `from` going up by what each call took.
    fn entries(&mut self, from: usize, limit: usize) -> Result<Vec<Value>, String>;
    /// The next session event of the reading.
    fn event(&mut self, event: SessionEvent) -> Result<(), String>;
}

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
    /// Part way through listing that turn again: where its next page begins
    /// and the digest of what was listed of it so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paging: Option<Paging>,
}

/// Part way through listing a settled turn's items again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paging {
    /// The cursor Codex gave for the next page.
    pub cursor: String,
    /// The entries listed so far, folded (`fold`).
    pub folded: u64,
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
    /// Read whole with somewhere to keep it: the events went there, none
    /// are here.
    pub kept: bool,
}

/// Reads `thread` past `seam` (from its start when the seam has no turn).
/// `accept` says whether every turn the World saw begin before a time had
/// ended by then. A whole reading is kept in `keeping`, where given.
pub(super) async fn tail<'k>(
    server: &AppServer,
    thread: &str,
    running: Option<&str>,
    owner: Option<&fastctx_types::Address>,
    seam: &Seam,
    accept: &(dyn Fn(Timestamp) -> bool + Send + Sync),
    mut keeping: Option<&mut (dyn Keeping + 'k)>,
) -> Result<Result<Tail, Restart>, String> {
    // Read again until two readings of what the thread is like now agree
    // around the reading of its entries (C3): the RPCs share no snapshot.
    for _ in 0..STEADY_TRIES {
        let before = shape(server, thread).await?;
        let read = once(server, thread, running, owner, seam, accept, keeping.as_deref_mut()).await?;
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
    keeping: Option<&mut (dyn Keeping + '_)>,
) -> Result<Result<Tail, Restart>, String> {
    let answer = server.request("thread/read", json!({"threadId": thread, "includeTurns": false})).await?;
    let metadata = &answer["thread"];
    if metadata.get("historyMode").and_then(Value::as_str) != Some("paginated") {
        return Ok(Err(Restart::Legacy));
    }
    let whole = seam.turn.is_none();
    if whole && let Some(keeping) = keeping {
        return kept(server, thread, running, owner, metadata, seam, accept, keeping).await;
    }
    let read = match &seam.turn {
        None => whole_entries(server, thread).await?,
        Some(turn) => match window_entries(server, thread, turn, seam.position).await? {
            Some(read) => read,
            None => return Ok(Err(Restart::Moved)),
        },
    };
    let Some(Read { timeline, items, voice }) = read else {
        // Not begun yet: nothing but its title.
        return Ok(Ok(Tail { events: history::title(metadata), whole, since: None, seam: Seam::default(), settled: None, kept: false }));
    };
    // One page of a settled turn listed again, the turns in turn; a turn is
    // compared once its last page is in.
    let (mut rotation, mut paging) = (seam.rotation, None);
    if !whole && !seam.settled.is_empty() {
        let at = rotation % seam.settled.len();
        let (turn, digest) = &seam.settled[at];
        let from = seam.paging.as_ref();
        let mut params = json!({"threadId": thread, "turnId": turn, "sortDirection": "asc", "limit": ITEMS_PAGE});
        if let Some(from) = from {
            params["cursor"] = json!(from.cursor);
        }
        match server.ask("thread/items/list", params).await {
            Ok(page) => {
                let data = page.get("data").and_then(Value::as_array).ok_or("thread/items/list answered without data")?;
                let folded = data.iter().fold(from.map_or(0, |from| from.folded), fold);
                match page.get("nextCursor").and_then(Value::as_str) {
                    Some(cursor) => {
                        paging = Some(Paging { cursor: cursor.to_string(), folded });
                        rotation = at;
                    }
                    None if folded != *digest => return Ok(Err(Restart::Changed(turn.clone()))),
                    None => rotation = at + 1,
                }
            }
            // A cursor kept from an earlier reading that Codex refuses now:
            // the turn is listed whole in this reading, as before it came a
            // page at a time — a turn whose cursors outlive no reading is
            // compared all the same, and the turns after it in their turn.
            Err(Unanswered::Rpc { .. }) if from.is_some() => {
                let whole = history::pages(server, "thread/items/list", json!({"threadId": thread, "turnId": turn, "sortDirection": "asc", "limit": ITEMS_PAGE}))
                    .await
                    .map_err(|error| error.to_string())?;
                if whole.iter().fold(0, fold) != *digest {
                    return Ok(Err(Restart::Changed(turn.clone())));
                }
                rotation = at + 1;
            }
            Err(error) => return Err(error.to_string()),
        }
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
    Ok(Ok(Tail { events, whole, since, seam: Seam { rotation, paging, ..next }, settled, kept: false }))
}

/// A whole reading, kept in `keeping` as it is read and its events handed
/// there (design §28.51): the reading `once` makes of `whole_entries`, the
/// same entries projected in the same order, not held whole in memory.
#[allow(clippy::too_many_arguments)]
async fn kept(
    server: &AppServer,
    thread: &str,
    running: Option<&str>,
    owner: Option<&fastctx_types::Address>,
    metadata: &Value,
    seam: &Seam,
    accept: &(dyn Fn(Timestamp) -> bool + Send + Sync),
    keeping: &mut dyn Keeping,
) -> Result<Result<Tail, Restart>, String> {
    keeping.begin()?;
    let Some((voice, digests)) = kept_entries(server, thread, keeping).await? else {
        // Not begun yet: nothing but its title.
        for event in history::title(metadata) {
            keeping.event(event)?;
        }
        return Ok(Ok(Tail { events: Vec::new(), whole: true, since: None, seam: Seam::default(), settled: None, kept: true }));
    };
    let created = super::seconds(metadata.get("createdAt"));
    // As a whole reading has it: a voice session open at the thread's start.
    if let Some(session) = voice {
        keeping.event(history::voice(created, session))?;
    }
    let mut projector = history::Projector::new(running, owner, created);
    let mut events = Vec::new();
    let mut from = 0;
    loop {
        let entries = keeping.entries(from, KEPT_AT_ONCE)?;
        if entries.is_empty() {
            break;
        }
        from += entries.len();
        for entry in &entries {
            projector.push(entry, &mut |turn, id| keeping.timing(turn, id), &mut events)?;
            for event in events.drain(..) {
                keeping.event(event)?;
            }
        }
    }
    let turns = projector.finish();
    for event in history::title(metadata) {
        keeping.event(event)?;
    }
    let (next, settled) = advance(seam, &turns, &digests, running, accept, true);
    Ok(Ok(Tail { events: Vec::new(), whole: true, since: None, seam: Seam { rotation: seam.rotation, paging: None, ..next }, settled, kept: true }))
}

/// What `whole_entries` lists, kept in `keeping` as it comes: when each item
/// began and was done, each turn's digest of its item entries folded as they
/// come, and the timeline's entries; for a thread whose timeline Codex will
/// not list, its item entries whole and the timeline made of them and its
/// turns. With the voice session open at the timeline's start. None before
/// the thread's first words.
async fn kept_entries(server: &AppServer, thread: &str, keeping: &mut dyn Keeping) -> Result<Option<(Option<String>, BTreeMap<String, u64>)>, String> {
    // Whether it has begun, as its turns tell: the turns themselves are
    // listed again a page at a time only where the timeline is made of
    // them, so that none is held through the reading.
    let turns = json!({"threadId":thread,"sortDirection":"asc","itemsView":"notLoaded"});
    let mut first = turns.clone();
    first["limit"] = json!(1);
    match server.ask("thread/turns/list", first).await {
        Ok(_) => {}
        Err(Unanswered::Rpc { code: -32600, message, data })
            if data.is_null()
                && message == format!("thread {thread} is not materialized yet; thread/turns/list is unavailable before first user message") =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    }
    // Kept whole only once the timeline is refused: the items are listed
    // again then, and all kept before is let go.
    let mut whole_items = false;
    loop {
        let mut digests: BTreeMap<String, u64> = BTreeMap::new();
        history::each_page(server, "thread/items/list", json!({"threadId": thread, "sortDirection": "asc", "limit": ITEMS_PAGE}), &mut |entry| {
            if let Some(turn) = entry["turnId"].as_str() {
                let running = digests.entry(turn.to_string()).or_insert(0);
                *running = fold(*running, &entry);
            }
            if let (Some(turn), Some(id)) = (entry["turnId"].as_str(), entry["item"]["id"].as_str()) {
                keeping.time(turn, id, Timing::of(&entry))?;
            }
            if whole_items {
                keeping.item(&entry["turnId"].to_string(), entry)?;
            }
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())?;
        if whole_items {
            ordinary_kept(server, turns.clone(), keeping).await?;
            return Ok(Some((None, digests)));
        }
        match history::each_page(server, "thread/timeline/list", json!({"threadId": thread}), &mut |entry| keeping.entry(entry)).await {
            Ok(voice) => return Ok(Some((voice, digests))),
            // The 0.160 timeline RPC excludes archived threads, and a Codex
            // may refuse it otherwise or not have it at all; its ordinary
            // turns and items give the whole history but voice.
            Err(Unanswered::Rpc { code: -32600 | -32601, .. }) => {
                keeping.begin()?;
                whole_items = true;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// The timeline `history::ordinary_timeline` makes of the turns (`turns`,
/// listed a page at a time) and the item entries kept whole, kept in its
/// turn.
async fn ordinary_kept(server: &AppServer, turns: Value, keeping: &mut dyn Keeping) -> Result<(), String> {
    let mut position = 0u64;
    let mut keep = |keeping: &mut dyn Keeping, mut entry: Value| {
        entry["position"] = json!(position);
        position += 1;
        keeping.entry(entry)
    };
    history::each_page(server, "thread/turns/list", turns, &mut |turn| {
        keep(&mut *keeping, json!({"type":"turnStarted","turnId":turn["id"],"startedAt":turn["startedAt"]}))?;
        let id = turn["id"].to_string();
        let mut from = 0;
        loop {
            let entries = keeping.items_of(&id, from, KEPT_AT_ONCE)?;
            if entries.is_empty() {
                break;
            }
            from += entries.len();
            for mut entry in entries {
                keep(&mut *keeping, json!({"type":"item","turnId":turn["id"],"item":entry["item"].take()}))?;
            }
        }
        if turn["status"] != "inProgress" {
            let mut end = turn;
            end["type"] = json!("turnCompleted");
            end["turnId"] = end["id"].clone();
            keep(&mut *keeping, end)?;
        }
        Ok(())
    })
    .await
    .map(drop)
    .map_err(|error| error.to_string())
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
    let items = history::pages(server, "thread/items/list", json!({"threadId": thread, "sortDirection": "asc", "limit": ITEMS_PAGE}))
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
        let mut page = match server.ask("thread/timeline/list", params.clone()).await {
            Ok(page) => page,
            Err(Unanswered::Rpc { code: -32600 | -32601, .. }) => return ordinary_window(server, thread, turn).await,
            Err(error) => return Err(error.to_string()),
        };
        let Some(Value::Array(data)) = page.get_mut("data").map(Value::take) else {
            return Err("thread/timeline/list answered without data".into());
        };
        let begins = data.iter().find(|entry| entry["type"] == "turnStarted" && entry["turnId"] == turn).and_then(|entry| entry["position"].as_u64());
        timeline.extend(data);
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
    history::pages(server, "thread/items/list", json!({"threadId": thread, "turnId": turn, "sortDirection": "asc", "limit": ITEMS_PAGE}))
        .await
        .map_err(|error| error.to_string())
}

/// The digest of a turn's item entries as listed: each folded in, in order,
/// so that a listing taken a page at a time comes to the same.
fn digest(entries: &[Value]) -> u64 {
    entries.iter().fold(0, fold)
}

/// `running` with `entry` folded in.
fn fold(running: u64, entry: &Value) -> u64 {
    xxhash_rust::xxh3::xxh3_64_with_seed(&serde_json::to_vec(entry).unwrap_or_default(), running)
}

/// Each turn's digest of the item entries `items` list.
fn digests(items: &[Value]) -> BTreeMap<String, u64> {
    let mut by_turn: BTreeMap<String, u64> = BTreeMap::new();
    for entry in items {
        if let Some(turn) = entry["turnId"].as_str() {
            let running = by_turn.entry(turn.to_string()).or_insert(0);
            *running = fold(*running, entry);
        }
    }
    by_turn
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
    let seam = Seam {
        turn: Some(chosen.id.clone()),
        position: chosen.position,
        before: latest[best],
        settled,
        rotation: base.rotation,
        paging: base.paging,
    };
    (seam, Some(chosen.start))
}
