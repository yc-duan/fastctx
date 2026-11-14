//! §18's Codex rows: the hosts' Codex follower against a fake app-server
//! that keeps one thread in pages, as Codex 0.160 serves it — WebSocket over
//! a Unix socket, the timeline read back from its newest entry, a turn's
//! items listed by turn, 100 entries a page. Items are 1–4 KiB of fixed-seed
//! text; one in ten is a command whose output is 64 KiB.
//!
//! - `codex-advance`: a settled turn of 2,000 items (the one the follower
//!   lists again each reading) and a turn under way that grows by one item
//!   before each reading, to about 200; timed from the item's being there to
//!   the reading's events committed, with this process's resident memory.
//! - `codex-relist`: a thread of 500 turns, 20,000 items, read whole once.
//!
//! The fake is this program, started by the follower's own client with
//! `app-server --listen unix://…`; what it serves is chosen by the
//! environment (`FAKE_CODEX_SHAPE`, and for a growing thread the file that
//! says how many items the turn under way has, `FAKE_CODEX_CONTROL`).

use crate::fixture::Seeded;
use crate::stats::Samples;
use crate::storage::{SCOPE, Scale, error, three};
use fastctx_content::v2::{Commit, EVENT, Kind as _, KindRecord, OVERSIZED, Timeline};
use fastctx_content::{Record, State};
use fastctx_hosts::codex::drill::{self, AppServer, Seam};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const THREAD: &str = "th-bench";
const PAGE: usize = 100;
const RECORD_BYTES: usize = 64 << 10;

/// A turn of the fake thread: its id, items and whether it ended.
struct Turn {
    id: String,
    items: Vec<Value>,
    completed: bool,
}

fn item(turn: usize, n: usize, seeded: &mut Seeded) -> Value {
    let id = format!("i{turn}-{n}");
    if n % 10 == 9 {
        json!({"type": "commandExecution", "id": id, "command": "cat build.log", "cwd": "/work",
               "aggregatedOutput": seeded.text(64 << 10), "exitCode": 0})
    } else {
        let length = seeded.between(1024, 4096);
        json!({"type": "agentMessage", "id": id, "text": seeded.text(length)})
    }
}

fn turns(shape: &str) -> Vec<Turn> {
    let mut seeded = Seeded::new(26);
    match shape {
        "relist" => (0..500)
            .map(|t| Turn { id: format!("t{t}"), items: (0..40).map(|n| item(t, n, &mut seeded)).collect(), completed: true })
            .collect(),
        _ => vec![
            Turn { id: "t0".into(), items: (0..2000).map(|n| item(0, n, &mut seeded)).collect(), completed: true },
            Turn { id: "t1".into(), items: (0..400).map(|n| item(1, n, &mut seeded)).collect(), completed: false },
        ],
    }
}

/// The thread as it is now: each turn's listing, its items' entries and the
/// timeline from the newest entry back, made once for each size the turn
/// under way has.
struct Now {
    turns: Vec<Value>,
    items: Vec<(String, Value)>,
    newest_first: Vec<Value>,
    updated: i64,
}

fn now(turns: &[Turn], growing: Option<usize>) -> Now {
    let (mut listed, mut items, mut timeline) = (Vec::new(), Vec::new(), Vec::new());
    let mut position = 0u64;
    for (index, turn) in turns.iter().enumerate() {
        let count = if turn.completed { turn.items.len() } else { growing.unwrap_or(turn.items.len()).min(turn.items.len()) };
        let started = 1_000_000 + index as i64 * 100;
        listed.push(if turn.completed {
            json!({"id": turn.id, "status": "completed", "startedAt": started, "completedAt": started + 50, "durationMs": 50_000, "error": null})
        } else {
            json!({"id": turn.id, "status": "inProgress", "startedAt": started, "completedAt": null, "durationMs": null, "error": null})
        });
        timeline.push(json!({"type": "turnStarted", "turnId": turn.id, "startedAt": started, "position": position}));
        position += 1;
        for (n, item) in turn.items[..count].iter().enumerate() {
            timeline.push(json!({"type": "item", "turnId": turn.id, "item": item, "position": position}));
            position += 1;
            let at = started * 1000 + n as i64;
            items.push((turn.id.clone(), json!({"turnId": turn.id, "item": item, "startedAtMs": at, "completedAtMs": at + 1})));
        }
        if turn.completed {
            timeline.push(json!({"type": "turnCompleted", "turnId": turn.id, "completedAt": started + 50, "status": "completed",
                                 "error": null, "durationMs": 50_000, "position": position}));
            position += 1;
        }
    }
    timeline.reverse();
    Now { turns: listed, items, newest_first: timeline, updated: 2_000_000 + growing.unwrap_or(0) as i64 }
}

fn page(entries: &[Value], cursor: Option<&str>, size: usize) -> Value {
    let from = cursor.and_then(|cursor| cursor.parse::<usize>().ok()).unwrap_or(0);
    let to = (from + size).min(entries.len());
    let next = (to < entries.len()).then(|| to.to_string());
    json!({"data": entries.get(from..to).unwrap_or_default(), "nextCursor": next})
}

fn answer(now: &Now, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    let cursor = params.get("cursor").and_then(Value::as_str);
    let size = params.get("limit").and_then(Value::as_u64).map_or(PAGE, |limit| limit as usize);
    match method {
        "initialize" => Ok(json!({"userAgent": "codex_cli_rs/0.160.0 (fastctx bench)"})),
        "thread/read" => {
            Ok(json!({"thread": {"id": THREAD, "historyMode": "paginated", "createdAt": 999_000, "updatedAt": now.updated, "name": null}}))
        }
        // Read back from the newest entry.
        "thread/timeline/list" => Ok(page(&now.newest_first, cursor, size)),
        "thread/turns/list" => {
            if params.get("sortDirection").and_then(Value::as_str) == Some("desc") {
                let newest_first: Vec<Value> = now.turns.iter().rev().cloned().collect();
                Ok(page(&newest_first, cursor, size))
            } else {
                Ok(page(&now.turns, cursor, size))
            }
        }
        "thread/items/list" => {
            let only = params.get("turnId").and_then(Value::as_str);
            let listed: Vec<Value> =
                now.items.iter().filter(|(turn, _)| only.is_none_or(|only| only == turn)).map(|(_, entry)| entry.clone()).collect();
            Ok(page(&listed, cursor, PAGE))
        }
        other => Err((-32601, format!("{other} is not served by the bench's fake"))),
    }
}

/// The fake app-server: listens where it is told, says so as Codex does,
/// and answers one connection until it closes.
pub(crate) fn serve(args: &[String]) -> Result<(), String> {
    use tokio_tungstenite::tungstenite::{Message, accept};
    let listen = args.windows(2).find(|pair| pair[0] == "--listen").map(|pair| pair[1].clone()).ok_or("no --listen")?;
    let path = listen.strip_prefix("unix://").ok_or("--listen is not unix://")?.to_string();
    let shape = std::env::var("FAKE_CODEX_SHAPE").unwrap_or_else(|_| "advance".into());
    let control = std::env::var_os("FAKE_CODEX_CONTROL").map(std::path::PathBuf::from);
    let turns = turns(&shape);
    let _ = std::fs::remove_file(&path);
    #[cfg(windows)]
    let listener = uds_windows::UnixListener::bind(&path).map_err(error)?;
    #[cfg(unix)]
    let listener = std::os::unix::net::UnixListener::bind(&path).map_err(error)?;
    eprintln!("{}", json!({"target": "codex_app_server_transport::transport::unix_socket", "fields": {"message": "app-server control socket listening"}}));
    let debug = std::env::var_os("BENCH_DEBUG").is_some();
    if debug {
        eprintln!("fake: bound at {path}");
    }
    let (stream, _) = listener.accept().map_err(error)?;
    if debug {
        eprintln!("fake: a connection came");
    }
    let mut socket = accept(stream).map_err(|failure| failure.to_string())?;
    if debug {
        eprintln!("fake: upgraded to a WebSocket");
    }
    let mut made: Option<(Option<usize>, Now)> = None;
    loop {
        let text = match socket.read() {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Binary(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            Ok(Message::Close(_)) | Err(_) => return Ok(()),
            Ok(_) => continue,
        };
        if std::env::var_os("BENCH_DEBUG").is_some() {
            eprintln!("fake got: {}", text.chars().take(200).collect::<String>());
        }
        let Ok(request) = serde_json::from_str::<Value>(&text) else { continue };
        let (Some(id), Some(method)) = (request.get("id").cloned(), request.get("method").and_then(Value::as_str)) else { continue };
        let growing = control
            .as_deref()
            .map(|path| std::fs::read_to_string(path).ok().and_then(|text| text.trim().parse::<usize>().ok()).unwrap_or(1));
        if made.as_ref().is_none_or(|(size, _)| *size != growing) {
            made = Some((growing, now(&turns, growing)));
        }
        let (_, current) = made.as_ref().expect("made above");
        let reply = match answer(current, method, &request["params"]) {
            Ok(result) => json!({"id": id, "result": result}),
            Err((code, message)) => json!({"id": id, "error": {"code": code, "message": message, "data": null}}),
        };
        let sent = socket.send(Message::Text(reply.to_string().into()));
        if debug {
            eprintln!("fake sent {method}: {sent:?}");
        }
        if sent.is_err() {
            return Ok(());
        }
    }
}

/// The follower's client, started on this program as its app-server, greeted.
async fn client(dir: &Path, shape: &str, control: Option<&Path>) -> Result<Arc<AppServer>, String> {
    let program = std::env::current_exe().map_err(error)?;
    let cwd = dir.join("codex");
    std::fs::create_dir_all(&cwd).map_err(error)?;
    // The client starts its app-server with only what it is given: the
    // system's own settings come along so its sockets can start.
    let mut env: Vec<(OsString, OsString)> = ["SystemRoot", "windir", "PATH", "TEMP", "TMP", "HOME", "USERPROFILE", "BENCH_DEBUG"]
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value)))
        .collect();
    env.push(("FAKE_CODEX_SHAPE".into(), shape.into()));
    if let Some(control) = control {
        env.push(("FAKE_CODEX_CONTROL".into(), control.as_os_str().to_owned()));
    }
    let debug = std::env::var_os("BENCH_DEBUG").is_some();
    let log: fastctx_hosts::codex::Log = Arc::new(move |line: String| {
        if debug {
            eprintln!("client: {line}");
        }
    });
    let server = drill::spawn(&program, &["app-server".to_string()], &cwd, &env, log).map_err(error)?;
    server
        .request("initialize", json!({"clientInfo": {"name": "fastctx", "title": "FastCtx World", "version": "bench"}, "capabilities": {"experimentalApi": true}}))
        .await?;
    server.notify("initialized", json!({}))?;
    Ok(server)
}

/// Commits `events` to the producer's timeline as the node keeps them: an
/// event too large for a record an object.
fn commit(three: &crate::storage::Three, events: &[fastctx_types::session::SessionEvent], number: &mut u64) -> Result<(), String> {
    let holding = three.producer.holding(SCOPE).map_err(error)?.ok_or("not held")?;
    for part in events.chunks(256) {
        let (mut records, mut leases) = (Vec::new(), Vec::new());
        for event in part {
            *number += 1;
            let encoded = serde_json::to_vec(event).map_err(error)?;
            let (body, objects) = if encoded.len() < RECORD_BYTES {
                let mut body = vec![EVENT];
                body.extend_from_slice(&encoded);
                (body, Vec::new())
            } else {
                let lease = three.producer.put(&mut encoded.as_slice(), "bench", Duration::from_secs(600)).map_err(error)?;
                let mut body = vec![OVERSIZED];
                body.extend_from_slice(&serde_json::to_vec(&json!({ "bytes": encoded.len(), "slim": null })).map_err(error)?);
                let objects = vec![lease.object];
                leases.push(lease);
                (body, objects)
            };
            let key = number.to_be_bytes().to_vec();
            records.push(KindRecord { kind: Timeline::KIND, schema: Timeline::SCHEMA, record: Record { item: key.clone(), order: key, state: State::Is { body, objects } } });
        }
        let commit = Commit { scope: SCOPE.into(), stream: Timeline::STREAM.into(), records, leases, durable: false, op: None, holding };
        three.producer.raw_commit(commit, three.device.clone()).map_err(error)?;
    }
    Ok(())
}

/// The most resident memory above `before` seen by a sampler every 5 ms.
struct Sampled {
    most: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
    before: u64,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Sampled {
    fn start() -> Result<Self, String> {
        let before = fastctx_platform::memory::resident().map_err(error)?;
        let (most, done) = (Arc::new(AtomicU64::new(before)), Arc::new(AtomicBool::new(false)));
        let thread = {
            let (most, done) = (most.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if let Ok(now) = fastctx_platform::memory::resident() {
                        most.fetch_max(now, Ordering::Relaxed);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        Ok(Self { most, done, before, thread: Some(thread) })
    }

    fn grew_mib(mut self) -> f64 {
        self.done.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.most.load(Ordering::Relaxed).saturating_sub(self.before) as f64 / f64::from(1 << 20)
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().map_err(error)
}

pub(crate) fn advance(dir: &Path, scale: Scale) -> Result<bool, String> {
    let rounds = match scale {
        Scale::Full => 200,
        Scale::Small => 40,
    };
    let control = dir.join("grow");
    std::fs::write(&control, "1").map_err(error)?;
    let three = three(dir)?;
    let accept = |_: jiff::Timestamp| true;
    runtime()?.block_on(async {
        let server = client(dir, "advance", Some(&control)).await?;
        let first = drill::tail(&server, THREAD, &Seam::default(), &accept).await?.map_err(|why| format!("the first reading restarted: {why}"))?;
        let mut number = 0u64;
        commit(&three, &first.events, &mut number)?;
        let mut seam = first.seam;
        let (mut samples, mut restarts) = (Samples::default(), 0u32);
        let sampled = Sampled::start()?;
        for round in 0..rounds + 20 {
            std::fs::write(&control, (2 + round).to_string()).map_err(error)?;
            let started = Instant::now();
            let tail = match drill::tail(&server, THREAD, &seam, &accept).await? {
                Ok(tail) => tail,
                Err(_) => {
                    restarts += 1;
                    samples.failed += 1;
                    continue;
                }
            };
            commit(&three, &tail.events, &mut number)?;
            if round >= 20 {
                samples.push(started.elapsed());
            }
            seam = tail.seam;
        }
        let grew = sampled.grew_mib();
        let met = samples.report(
            "codex-advance",
            "an item more in the turn under way, read past the seam with the settled turn of 2,000 listed again, committed",
            Some(50.0),
            json!({ "grew_mib": grew, "target_mib": 64.0, "restarts": restarts }),
        );
        Ok(met && grew <= 64.0)
    })
}

pub(crate) fn relist(dir: &Path, _scale: Scale) -> Result<bool, String> {
    let three = three(dir)?;
    let accept = |_: jiff::Timestamp| true;
    runtime()?.block_on(async {
        let server = client(dir, "relist", None).await?;
        let sampled = Sampled::start()?;
        let started = Instant::now();
        let whole = drill::tail(&server, THREAD, &Seam::default(), &accept).await?.map_err(|why| format!("the reading restarted: {why}"))?;
        let mut number = 0u64;
        commit(&three, &whole.events, &mut number)?;
        let took = started.elapsed();
        let grew = sampled.grew_mib();
        let met = took <= Duration::from_secs(10) && grew <= 64.0;
        println!(
            "{}",
            json!({ "row": "codex-relist", "measure": "a thread of 500 turns, 20,000 items, read whole and committed",
                    "seconds": took.as_secs_f64(), "grew_mib": grew, "events": whole.events.len(), "target_seconds": 10.0, "target_mib": 64.0, "met": met })
        );
        Ok(met)
    })
}
