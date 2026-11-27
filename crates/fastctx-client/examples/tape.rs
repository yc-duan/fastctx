//! Records what a conversation's page is given, as the application's shell
//! gives it (`gui/src-tauri/src/replica.rs`): the latest part of its
//! timeline, each record the timeline gains, and what its live watch tells,
//! one JSON line each with the milliseconds since the recording began. It
//! only reads. The page's own fold can then be run over the recording
//! (`gui/scripts/conversation-bench`).
//!
//! ```text
//! tape <resident> <machine> <out.jsonl> [--seconds N]
//! ```

use fastctx_client::World;
use fastctx_types::ResidentId;
use fastctx_types::access::Written;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::session::{Happened, Item, SessionChange, SessionEvent, Watched, What};
use fastctx_types::timeline_pages::{
    PAGE_BYTES, PAGE_EVENTS, TimelineAsk, TimelineChange, TimelineEvent, TimelineFrom, TimelineShown, TimelineTold,
};
use serde_json::{Value, json};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Tape {
    began: Instant,
    out: Mutex<std::fs::File>,
}

impl Tape {
    fn put(&self, piece: Value) {
        let line = json!({ "t": self.began.elapsed().as_millis() as u64, "piece": piece });
        let mut out = self.out.lock().expect("tape");
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// As the shell decodes an event of a page: an event, or one cut short.
fn decoded(raw: &TimelineEvent) -> Option<(SessionEvent, Option<Value>)> {
    match &raw.shown {
        TimelineShown::Event { event } => Some((serde_json::from_value(event.clone()).ok()?, None)),
        TimelineShown::Oversized { bytes, slim, object } => {
            let deferred = Some(json!({ "bytes": bytes, "object": object }));
            let event = match slim {
                Some(slim) => serde_json::from_value(slim.clone()).ok()?,
                None => SessionEvent {
                    at: jiff::Timestamp::UNIX_EPOCH,
                    happened: Happened::Finished {
                        turn: format!("timeline:{}", raw.place.item),
                        item: Item {
                            id: raw.place.item.clone(),
                            draft: None,
                            started_at: None,
                            detail: None,
                            what: What::Other { host_kind: "fastctx:timeline-preview-unavailable".into() },
                        },
                    },
                },
            };
            Some((event, deferred))
        }
    }
}

fn shown(raw: &TimelineEvent, event: &SessionEvent, deferred: Option<Value>) -> Value {
    let mut shown = json!({ "key": raw.place.item, "place": raw.place, "event": event });
    if let Some(deferred) = deferred {
        shown["deferred"] = deferred;
    }
    shown
}

fn boundary(event: &SessionEvent) -> bool {
    matches!(
        &event.happened,
        Happened::TurnStarted { .. }
            | Happened::Heard { .. }
            | Happened::Finished { item: Item { what: What::Message { .. }, .. }, .. }
    )
}

/// The latest part, as the shell cuts it; where the timeline stood.
async fn latest(
    world: &World,
    resident: &ResidentId,
    machine: &Written,
) -> Result<(Value, Option<u64>, Option<u64>), String> {
    let told = world
        .timeline(
            Some(machine.clone()),
            TimelineAsk::Page {
                resident: resident.clone(),
                from: TimelineFrom::Latest,
                max_events: PAGE_EVENTS,
                max_bytes: PAGE_BYTES,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    let TimelineTold::Page { revision, initializing, events, older, newer, through } = told else {
        return Err("the timeline did not answer a page".into());
    };
    let ends = events.first().map(|raw| raw.place.clone());
    let mut read: Vec<(TimelineEvent, SessionEvent, Option<Value>)> =
        events.into_iter().filter_map(|raw| decoded(&raw).map(|(event, deferred)| (raw, event, deferred))).collect();
    let mut older = older;
    if older.is_some()
        && let Some(at) = read.iter().position(|(_, event, _)| boundary(event))
        && at > 0
    {
        read.drain(..at);
    }
    if older.is_some() {
        older = read.first().map(|(raw, _, _)| raw.place.clone()).or(ends);
    }
    let events: Vec<Value> = read.iter().map(|(raw, event, deferred)| shown(raw, event, deferred.clone())).collect();
    let preparing = initializing.unwrap_or(revision.is_none());
    Ok((
        json!({ "generation": 0, "events": events, "older": older, "newer": newer, "preparing": preparing }),
        revision,
        through,
    ))
}

fn waits(event: &SessionEvent) -> bool {
    matches!(
        event.happened,
        Happened::Changed { change: SessionChange::Request { .. } | SessionChange::Resolved { .. }, .. }
    )
}

fn shows(event: &SessionEvent) -> bool {
    matches!(
        event.happened,
        Happened::Began { .. }
            | Happened::Updated { .. }
            | Happened::Finished { .. }
            | Happened::Grew { part: fastctx_types::session::Part::Text, .. }
    )
}

async fn records(
    world: World,
    resident: ResidentId,
    machine: Written,
    tape: Arc<Tape>,
    mut revision: Option<u64>,
    mut through: Option<u64>,
) {
    loop {
        let moved = world
            .timeline(
                Some(machine.clone()),
                TimelineAsk::Moved { resident: resident.clone(), after: revision.unwrap_or(0) },
            )
            .await;
        let now = match moved {
            Ok(TimelineTold::Moved { revision, initializing }) => {
                if let Some(initializing) = initializing {
                    tape.put(json!({ "kind": "moved", "revision": revision, "initializing": initializing }));
                }
                revision
            }
            other => {
                tape.put(json!({ "kind": "note", "said": format!("moved failed: {:?}", other.err()) }));
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        if now == revision {
            continue;
        }
        let Some(mut after) = through else {
            match reread(&world, &resident, &machine, &tape).await {
                Some((now, at)) => (revision, through) = (now, at),
                None => return,
            }
            continue;
        };
        loop {
            let asked = TimelineAsk::Changes {
                resident: resident.clone(),
                after,
                max_events: PAGE_EVENTS,
                max_bytes: PAGE_BYTES,
            };
            match world.timeline(Some(machine.clone()), asked).await {
                Ok(TimelineTold::Changes { revision: read, changes, through: next, more, .. }) => {
                    let mut told = Vec::new();
                    for change in changes {
                        match change {
                            TimelineChange::Gone { item } => told.push(json!({ "record": "gone", "key": item })),
                            TimelineChange::Is { event: raw } => {
                                if let Some((event, deferred)) = decoded(&raw) {
                                    told.push(json!({ "record": "is", "shown": shown(&raw, &event, deferred) }));
                                }
                            }
                        }
                    }
                    if !told.is_empty() {
                        tape.put(json!({ "kind": "records", "generation": 0, "records": told }));
                    }
                    (after, through) = (next, Some(next));
                    if !more {
                        revision = read.or(now);
                        break;
                    }
                }
                Ok(TimelineTold::Reread) => {
                    match reread(&world, &resident, &machine, &tape).await {
                        Some((now, at)) => (revision, through) = (now, at),
                        None => return,
                    }
                    break;
                }
                other => {
                    tape.put(json!({ "kind": "note", "said": format!("changes failed: {:?}", other.err()) }));
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }
        }
    }
}

/// The timeline was made anew: as the shell does, the page is given the
/// latest part in place of all it holds.
async fn reread(
    world: &World,
    resident: &ResidentId,
    machine: &Written,
    tape: &Tape,
) -> Option<(Option<u64>, Option<u64>)> {
    for _ in 0..20 {
        match latest(world, resident, machine).await {
            Ok((part, revision, through)) => {
                tape.put(json!({ "kind": "replica", "part": part, "revision": revision, "through": through }));
                return Some((revision, through));
            }
            Err(error) => {
                tape.put(json!({ "kind": "note", "said": format!("reading anew failed: {error}") }));
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    None
}

async fn live(world: World, resident: ResidentId, machine: Written, tape: Arc<Tape>) {
    loop {
        let mut watching = match world.watch_live(resident.clone(), Some(machine.clone())).await {
            Ok(watching) => watching,
            Err(error) => {
                tape.put(json!({ "kind": "interrupted", "error": error.to_string() }));
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        let mut waiting: Option<Vec<Value>> = Some(Vec::new());
        loop {
            let next = match watching.next().await {
                Ok(Some(next)) => next,
                Ok(None) => {
                    tape.put(json!({ "kind": "interrupted", "error": "the live watch ended" }));
                    break;
                }
                Err(error) => {
                    tape.put(json!({ "kind": "interrupted", "error": error.to_string() }));
                    break;
                }
            };
            match next {
                Watched::Resumed => {
                    if let Some(events) = waiting.take() {
                        tape.put(json!({ "kind": "waiting", "events": events }));
                        tape.put(json!({ "kind": "resumed" }));
                    }
                }
                Watched::Live { event, .. } if waits(&event) => match &mut waiting {
                    Some(events) => events.push(serde_json::to_value(&event).unwrap_or_default()),
                    None => tape.put(json!({ "kind": "asked", "event": event })),
                },
                Watched::Now { busy, turn, kept, halted } => {
                    tape.put(json!({ "kind": "now", "busy": busy, "turn": turn, "kept": kept, "halted": halted }))
                }
                Watched::Live { event, .. } if shows(&event) => tape.put(json!({ "kind": "live", "event": event })),
                Watched::Live { event, .. } => tape.put(json!({ "kind": "unshown", "event": event })),
                _ => {}
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "tape <resident> <machine> <out.jsonl> [--seconds N]";
    let resident = ResidentId::try_from(args.next().expect(usage)).expect("a resident id");
    let machine: Written = args.next().expect(usage).parse().expect("a machine name");
    let out = args.next().expect(usage);
    let mut seconds = 600;
    while let Some(arg) = args.next() {
        if arg == "--seconds" {
            seconds = args.next().and_then(|value| value.parse().ok()).expect("--seconds N");
        }
    }
    let root = fastctx_platform::fastctx_root().expect("the World's home");
    let world = World::new(WorldDir::under(&root), Peer::Command { resident: None, session: None });
    let tape =
        Arc::new(Tape { began: Instant::now(), out: Mutex::new(std::fs::File::create(&out).expect("the tape file")) });
    let (part, revision, through) = latest(&world, &resident, &machine).await.expect("the latest part");
    tape.put(json!({ "kind": "replica", "part": part, "revision": revision, "through": through }));
    let watching = tokio::spawn(live(world.clone(), resident.clone(), machine.clone(), tape.clone()));
    let following =
        tokio::spawn(records(world.clone(), resident.clone(), machine.clone(), tape.clone(), revision, through));
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    // Where the timeline stands at the end, as a page opened now would read it.
    if let Ok((part, revision, through)) = latest(&world, &resident, &machine).await {
        tape.put(json!({ "kind": "final", "part": part, "revision": revision, "through": through }));
    }
    watching.abort();
    following.abort();
}
