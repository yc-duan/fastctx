use super::{ITEMS_PAGE, Read};
use crate::codex::{history, server::AppServer};
use fastctx_types::{Address, session::SessionEvent};
use jiff::Timestamp;
use serde_json::{Value, json};

const FIRST_PAGE: u32 = 4;
const PAGE: u32 = 100;
const PAGES_MOST: usize = 5;

type Digest = [u8; 32];

fn digest(value: &Value) -> Digest {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(value.to_string().as_bytes()).into()
}

/// The order of one row within an ordinary-plus-realtime rollout.
pub fn order(entry: &Value) -> Option<(u64, u8, &str)> {
    let position = entry["position"].as_u64()?;
    let (kind, id) = match entry["type"].as_str()? {
        "turnStarted" => (0, entry["turnId"].as_str()?),
        "item" => (1, entry.pointer("/item/id")?.as_str()?),
        "realtime" => (2, entry.pointer("/item/id")?.as_str()?),
        "turnCompleted" => (3, entry["turnId"].as_str()?),
        _ => return None,
    };
    Some((position, kind, id))
}

pub(in crate::codex) fn canonical(rows: &mut [Value]) {
    rows.sort_by(|a, b| order(a).cmp(&order(b)));
}

#[derive(Clone)]
struct Item {
    position: u64,
    id: String,
    timeline: Digest,
    listed: Digest,
}

impl Item {
    fn of(timeline: &Value, listed: &Value, turn: &str) -> Option<Self> {
        let (position, 1, id) = order(timeline)? else { return None };
        if timeline["turnId"] != turn || listed["turnId"] != turn || timeline["item"] != listed["item"] {
            return None;
        }
        if listed["completedAtMs"].as_i64().is_none()
            || timeline.pointer("/item/status").and_then(Value::as_str) == Some("inProgress")
        {
            return None;
        }
        Some(Self { position, id: id.into(), timeline: digest(timeline), listed: digest(listed) })
    }

    fn anchors(&self, row: &Value, turn: &str) -> bool {
        order(row) == Some((self.position, 1, self.id.as_str())) && row["turnId"] == turn
    }
}

/// An in-memory projection boundary within the running turn.
#[derive(Clone)]
pub struct Through {
    turn: String,
    path: String,
    last: Item,
    previous: Option<String>,
    latest: Timestamp,
    projection: history::RunningProjection,
}

impl Through {
    pub fn turn(&self) -> &str {
        &self.turn
    }
    pub fn latest(&self) -> Timestamp {
        self.latest
    }
}

pub(super) fn seed(
    metadata: &Value,
    read: &Read,
    projection: Option<history::RunningProjection>,
    latest: Option<Timestamp>,
) -> Option<Through> {
    if !read.canonical {
        return None;
    }
    let projection = projection?;
    let turn = &projection.span.id;
    let mut rows = read.items.iter().rev().filter(|row| row["turnId"] == turn.as_str());
    let listed = rows.next()?;
    let last = Item::of(read.timeline.last()?, listed, turn)?;
    let previous = previous(rows.next())?;
    Some(Through {
        turn: turn.clone(),
        path: metadata["path"].as_str()?.into(),
        last,
        previous,
        latest: latest?,
        projection,
    })
}

fn previous(row: Option<&Value>) -> Option<Option<String>> {
    match row {
        Some(row) => Some(Some(row.pointer("/item/id")?.as_str()?.into())),
        None => Some(None),
    }
}

pub struct Delta {
    pub through: Through,
    pub events: Vec<SessionEvent>,
    pub told: Vec<SessionEvent>,
}

pub(in crate::codex) async fn read(
    server: &AppServer,
    thread: &str,
    running: Option<&str>,
    owner: Option<&Address>,
    through: &Through,
) -> Result<Result<Delta, &'static str>, String> {
    if running != Some(through.turn.as_str()) {
        return Ok(Err("the running turn changed"));
    }
    let answer = server.request("thread/read", json!({"threadId":thread,"includeTurns":false})).await?;
    let metadata = &answer["thread"];
    if metadata["historyMode"] != "paginated" || metadata["path"] != through.path {
        return Ok(Err("the rollout changed"));
    }
    let mut request = json!({"threadId":thread,"limit":FIRST_PAGE});
    let mut chunks = Vec::new();
    let mut found = false;
    let mut newer = None;
    for _ in 0..PAGES_MOST {
        let response = match server.ask("thread/timeline/list", request.clone()).await {
            Ok(response) => response,
            Err(super::Unanswered::Rpc { .. }) => return Ok(Err("timeline is not readable")),
            Err(error) => return Err(error.to_string()),
        };
        let Some(rows) = response["data"].as_array() else { return Ok(Err("invalid timeline page")) };
        let mut newer_of_page =
            newer.as_ref().map(|(position, kind, id): &(u64, u8, String)| (*position, *kind, id.as_str()));
        let mut accepted = Vec::new();
        for row in rows.iter().rev() {
            let Some(key) = order(row) else { return Ok(Err("unrecognized timeline row")) };
            if newer_of_page.is_some_and(|newer| key >= newer) {
                return Ok(Err("timeline order changed"));
            }
            newer_of_page = Some(key);
            if through.last.anchors(row, &through.turn) {
                if through.last.timeline != digest(row) {
                    return Ok(Err("the timeline anchor changed"));
                }
                found = true;
                break;
            }
            if key.0 <= through.last.position {
                return Ok(Err("the timeline anchor is gone"));
            }
            if key.1 != 1 || row["turnId"] != through.turn {
                return Ok(Err("a timeline row crosses the running turn"));
            }
            accepted.push(row.clone());
        }
        newer = newer_of_page.map(|(position, kind, id)| (position, kind, id.to_owned()));
        chunks.extend(accepted);
        if found {
            break;
        }
        let Some(cursor) = response["nextCursor"].as_str() else { break };
        if request["cursor"] == cursor {
            return Ok(Err("repeated timeline cursor"));
        }
        request["cursor"] = json!(cursor);
        request["limit"] = json!(PAGE);
    }
    if !found {
        return Ok(Err("the timeline anchor is out of reach"));
    }
    chunks.reverse();

    let mut request = json!({"threadId":thread,"turnId":through.turn,"sortDirection":"asc","limit":ITEMS_PAGE});
    if let Some(previous) = &through.previous {
        request["cursor"] = json!({"type":"item","itemId":previous});
    }
    let mut listed = Vec::new();
    let mut ended = false;
    for _ in 0..PAGES_MOST {
        let response = match server.ask("thread/items/list", request.clone()).await {
            Ok(response) => response,
            Err(super::Unanswered::Rpc { .. }) => return Ok(Err("item anchor refused")),
            Err(error) => return Err(error.to_string()),
        };
        let Some(rows) = response["data"].as_array() else { return Ok(Err("invalid item page")) };
        listed.extend(rows.iter().cloned());
        if listed.len() > chunks.len() + 1 {
            return Ok(Err("item listing grew beyond the timeline page"));
        }
        let Some(cursor) = response["nextCursor"].as_str() else {
            ended = true;
            break;
        };
        if request["cursor"] == cursor {
            return Ok(Err("repeated item cursor"));
        }
        request["cursor"] = json!(cursor);
    }
    if !ended || listed.len() != chunks.len() + 1 || digest(&listed[0]) != through.last.listed {
        return Ok(Err("item listing did not meet the timeline and its anchor"));
    }

    let mut next = through.clone();
    let mut projector = history::Projector::resume(&through.turn, owner, &through.projection);
    let mut events = Vec::new();
    for (row, listed) in chunks.iter().zip(&listed[1..]) {
        let Some(item) = Item::of(row, listed, &through.turn) else {
            return Ok(Err("an item is incomplete or differs between listings"));
        };
        if row.pointer("/item/type").and_then(Value::as_str) == Some("userMessage") {
            return Ok(Err("words steered into the running turn"));
        }
        let timing = super::Timing::of(listed);
        projector.push(
            row,
            &mut |turn, id| Ok((turn == through.turn && id == item.id).then_some(timing)),
            &mut events,
        )?;
        next.previous = Some(std::mem::replace(&mut next.last, item).id);
    }
    if let Some(latest) = events.iter().map(|event| event.at).max() {
        next.latest = next.latest.max(latest);
    }
    let Some(projection) = projector.running_projection() else {
        return Ok(Err("projection crossed the running turn"));
    };
    next.projection = projection;
    Ok(Ok(Delta { through: next, events, told: history::title(metadata) }))
}
