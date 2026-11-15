//! §18's Codex rows: the hosts' Codex follower against a fake app-server
//! that keeps one thread in pages, as Codex 0.160 serves it — WebSocket over
//! a Unix socket, the timeline read back from its newest entry, a turn's
//! items listed by turn, 100 entries a page. Items are 1–4 KiB of fixed-seed
//! text; one in ten is a command whose output is 64 KiB.
//!
//! - `codex-advance`: a settled turn of 2,000 items (the one the follower
//!   lists again each reading) and a turn under way that grows by one item
//!   before each reading, to about 200; timed from the item's being there to
//!   the reading written as the node writes a timeline (`fastctx_node::bench`:
//!   placed past the floor, each item of the window compared with what the
//!   library holds, the changed written, the gone removed, the checkpoint
//!   kept), with this process's resident memory.
//! - `codex-relist`: a thread of 500 turns, 20,000 items, read whole once,
//!   kept on disk as it is read and placed from there (design §28.51).
//! - `codex-kept-same`: the same readings placed as one held in memory and
//!   as one kept on disk, into two libraries, must leave the same timeline
//!   item for item, the same checkpoint and the same counts: a thread with
//!   a turn under way read whole, then the thread of 500 turns over it (a
//!   timeline with items gone and items kept), then that thread again (all
//!   compared, nothing written).
//!
//! The fake is this program, started by the follower's own client with
//! `app-server --listen unix://…`; what it serves is chosen by the
//! environment (`FAKE_CODEX_SHAPE`, and for a growing thread the file that
//! says how many items the turn under way has, `FAKE_CODEX_CONTROL`).

use crate::fixture::Seeded;
use crate::stats::Samples;
use crate::storage::{SCOPE, Scale, error, three};
use fastctx_hosts::codex::drill::{self, AppServer, Seam};
use fastctx_node::bench::Follower;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const THREAD: &str = "th-bench";
const PAGE: usize = 100;

/// A turn of the fake thread: its id, items and whether it ended.
struct Turn {
    id: String,
    items: Vec<Value>,
    completed: bool,
}

fn item(turn: usize, n: usize, seeded: &mut Seeded) -> Value {
    let id = format!("i{turn}-{n}");
    if n % 10 == 9 {
        // A float written to its last digit: read as JSON text and written
        // again, it may come back another, as a reading kept on disk must not.
        json!({"type": "commandExecution", "id": id, "command": "cat build.log", "cwd": "/work",
               "aggregatedOutput": seeded.text(64 << 10), "exitCode": 0, "durationSeconds": 0.000_000_216_756_040_498_884_38 * (n as f64 + 1.0)})
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

/// The whole thread, made once: each turn's listing, the item entries and
/// the timeline in their order, with the turn under way at its fullest.
/// What is there at a size of that turn is a prefix of each (the turn under
/// way is the last, and has no end), so nothing is made again or copied
/// whole for a request: a page copies its own entries only.
struct Thread {
    turns: Vec<Value>,
    items: Vec<(String, Value)>,
    /// Where each turn's entries lie in `items`.
    spans: Vec<(String, std::ops::Range<usize>)>,
    timeline: Vec<Value>,
    /// The turn under way: how many timeline entries and item entries come
    /// before its items.
    growing: Option<(usize, usize)>,
}

/// How much of the thread is there now.
struct Now {
    timeline: usize,
    items: usize,
    updated: i64,
}

fn thread(turns: &[Turn]) -> Thread {
    let (mut listed, mut items, mut spans, mut timeline) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut growing = None;
    let mut position = 0u64;
    for (index, turn) in turns.iter().enumerate() {
        assert!(turn.completed || index + 1 == turns.len(), "only the last turn is under way");
        let started = 1_000_000 + index as i64 * 100;
        listed.push(if turn.completed {
            json!({"id": turn.id, "status": "completed", "startedAt": started, "completedAt": started + 50, "durationMs": 50_000, "error": null})
        } else {
            json!({"id": turn.id, "status": "inProgress", "startedAt": started, "completedAt": null, "durationMs": null, "error": null})
        });
        timeline.push(json!({"type": "turnStarted", "turnId": turn.id, "startedAt": started, "position": position}));
        position += 1;
        if !turn.completed {
            growing = Some((timeline.len(), items.len()));
        }
        let first = items.len();
        for (n, item) in turn.items.iter().enumerate() {
            timeline.push(json!({"type": "item", "turnId": turn.id, "item": item, "position": position}));
            position += 1;
            let at = started * 1000 + n as i64;
            items.push((turn.id.clone(), json!({"turnId": turn.id, "item": item, "startedAtMs": at, "completedAtMs": at + 1})));
        }
        spans.push((turn.id.clone(), first..items.len()));
        if turn.completed {
            timeline.push(json!({"type": "turnCompleted", "turnId": turn.id, "completedAt": started + 50, "status": "completed",
                                 "error": null, "durationMs": 50_000, "position": position}));
            position += 1;
        }
    }
    Thread { turns: listed, items, spans, timeline, growing }
}

impl Thread {
    /// How much is there with `growing` items in the turn under way.
    fn now(&self, growing: Option<usize>) -> Now {
        let updated = 2_000_000 + growing.unwrap_or(0) as i64;
        match (self.growing, growing) {
            (Some((timeline, items)), Some(count)) => {
                let count = count.min(self.items.len() - items);
                Now { timeline: timeline + count, items: items + count, updated }
            }
            _ => Now { timeline: self.timeline.len(), items: self.items.len(), updated },
        }
    }
}

/// The page of `count` entries from the cursor, `at(index)` the entry at
/// each place; the next cursor where more are left.
fn page(count: usize, cursor: Option<&str>, size: usize, at: impl Fn(usize) -> Value) -> Value {
    let from = cursor.and_then(|cursor| cursor.parse::<usize>().ok()).unwrap_or(0).min(count);
    let to = (from + size).min(count);
    let next = (to < count).then(|| to.to_string());
    json!({"data": (from..to).map(at).collect::<Vec<Value>>(), "nextCursor": next})
}

fn answer(thread: &Thread, now: &Now, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    let cursor = params.get("cursor").and_then(Value::as_str);
    let size = params.get("limit").and_then(Value::as_u64).map_or(PAGE, |limit| limit as usize);
    match method {
        "initialize" => Ok(json!({"userAgent": "codex_cli_rs/0.160.0 (fastctx bench)"})),
        "thread/read" => {
            Ok(json!({"thread": {"id": THREAD, "historyMode": "paginated", "createdAt": 999_000, "updatedAt": now.updated, "name": null}}))
        }
        // Read back from the newest entry.
        "thread/timeline/list" => Ok(page(now.timeline, cursor, size, |index| thread.timeline[now.timeline - 1 - index].clone())),
        "thread/turns/list" => {
            let count = thread.turns.len();
            if params.get("sortDirection").and_then(Value::as_str) == Some("desc") {
                Ok(page(count, cursor, size, |index| thread.turns[count - 1 - index].clone()))
            } else {
                Ok(page(count, cursor, size, |index| thread.turns[index].clone()))
            }
        }
        "thread/items/list" => {
            let range = match params.get("turnId").and_then(Value::as_str) {
                Some(turn) => thread.spans.iter().find(|(id, _)| id == turn).map_or(0..0, |(_, range)| range.start.min(now.items)..range.end.min(now.items)),
                None => 0..now.items,
            };
            Ok(page(range.len(), cursor, PAGE, |index| thread.items[range.start + index].1.clone()))
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
    let thread = thread(&turns(&shape));
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
        let reply = match answer(&thread, &thread.now(growing), method, &request["params"]) {
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

/// The `at` quantile of `values`, in their order once sorted.
fn percentile(values: &mut [f64], at: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(((values.len() as f64 - 1.0) * at).round() as usize).copied().unwrap_or(0.0)
}

/// The most resident memory above `before` seen by a sampler every 5 ms.
pub(crate) struct Sampled {
    most: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
    before: u64,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Sampled {
    pub(crate) fn start() -> Result<Self, String> {
        let before = fastctx_platform::memory::resident().map_err(error)?;
        let (most, done) = (Arc::new(AtomicU64::new(before)), Arc::new(AtomicBool::new(false)));
        let thread = {
            let (most, done) = (most.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if let Ok(now) = fastctx_platform::memory::resident() {
                        most.fetch_max(now, Ordering::Relaxed);
                        fastctx_node::bench::note_resident(now);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        Ok(Self { most, done, before, thread: Some(thread) })
    }

    /// What was resident when it began.
    pub(crate) fn before(&self) -> u64 {
        self.before
    }

    pub(crate) fn grew_mib(mut self) -> f64 {
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
        let first = drill::tail(&server, THREAD, &Seam::default(), &accept, None).await?.map_err(|why| format!("the first reading restarted: {why}"))?;
        // Written as the node writes a timeline (`fastctx_node::bench`).
        let follower = Follower::new(three.producer.clone(), three.device.clone(), SCOPE)?;
        let mut seam = first.seam.clone();
        follower.read(first.events, first.whole, first.since, first.settled, first.seam)?;
        let (mut samples, mut restarts) = (Samples::default(), 0u32);
        // The host's part apart, its reading: what is left is the writing.
        let mut reading: Vec<f64> = Vec::new();
        let sampled = Sampled::start()?;
        for round in 0..rounds + 20 {
            std::fs::write(&control, (2 + round).to_string()).map_err(error)?;
            let started = Instant::now();
            // The history's version, as the node tells it before reading on.
            drill::read(&server, THREAD).await?;
            let tail = match drill::tail(&server, THREAD, &seam, &accept, None).await? {
                Ok(tail) => tail,
                Err(_) => {
                    restarts += 1;
                    samples.failed += 1;
                    continue;
                }
            };
            let read = started.elapsed();
            seam = tail.seam.clone();
            follower.read(tail.events, tail.whole, tail.since, tail.settled, tail.seam)?;
            if round >= 20 {
                samples.push(started.elapsed());
                reading.push(read.as_secs_f64() * 1000.0);
            }
        }
        let grew = sampled.grew_mib();
        let met = samples.report(
            "codex-advance",
            "an item more in the turn under way, read past the seam with the settled turn of 2,000 listed again, committed",
            Some(50.0),
            json!({ "grew_mib": grew, "target_mib": 64.0, "restarts": restarts, "reading_p50_ms": percentile(&mut reading, 0.5), "reading_p99_ms": percentile(&mut reading, 0.99) }),
        );
        Ok(met && grew <= 64.0)
    })
}

pub(crate) fn relist(dir: &Path, _scale: Scale) -> Result<bool, String> {
    let three = three(dir)?;
    let accept = |_: jiff::Timestamp| true;
    runtime()?.block_on(async {
        let server = client(dir, "relist", None).await?;
        let follower = Follower::new(three.producer.clone(), three.device.clone(), SCOPE)?;
        let sampled = Sampled::start()?;
        let started = Instant::now();
        // Kept beside the libraries, or where `BENCH_SPILL` says.
        let root = std::env::var_os("BENCH_SPILL").map(std::path::PathBuf::from).unwrap_or_else(|| dir.join("spill"));
        let mut keeping = follower.keeping(root);
        let whole = drill::tail(&server, THREAD, &Seam::default(), &accept, Some(&mut keeping)).await?.map_err(|why| format!("the reading restarted: {why}"))?;
        // The host's part apart: its reading and projection, kept on disk.
        let reading = started.elapsed();
        let (written, gone) = follower.read_kept(keeping, whole.settled, whole.seam)?;
        let took = started.elapsed();
        let grew = sampled.grew_mib();
        let met = took <= Duration::from_secs(10) && grew <= 64.0;
        let phases: serde_json::Map<String, Value> = follower.phases().into_iter().map(|(part, seconds)| (part.to_string(), json!(seconds))).collect();
        println!(
            "{}",
            json!({ "row": "codex-relist", "measure": "a thread of 500 turns, 20,000 items, read whole, kept on disk and committed",
                    "seconds": took.as_secs_f64(), "reading_seconds": reading.as_secs_f64(), "phases": phases, "grew_mib": grew, "written": written, "gone": gone,
                    "target_seconds": 10.0, "target_mib": 64.0, "met": met })
        );
        Ok(met)
    })
}

/// `codex-relist` as it was before readings were kept on disk: held whole
/// in memory, for comparison.
pub(crate) fn relist_held(dir: &Path, _scale: Scale) -> Result<bool, String> {
    let three = three(dir)?;
    let accept = |_: jiff::Timestamp| true;
    runtime()?.block_on(async {
        let server = client(dir, "relist", None).await?;
        let follower = Follower::new(three.producer.clone(), three.device.clone(), SCOPE)?;
        let sampled = Sampled::start()?;
        let started = Instant::now();
        let whole = drill::tail(&server, THREAD, &Seam::default(), &accept, None).await?.map_err(|why| format!("the reading restarted: {why}"))?;
        let reading = started.elapsed();
        let (written, gone) = follower.read(whole.events, true, whole.since, whole.settled, whole.seam)?;
        let took = started.elapsed();
        let grew = sampled.grew_mib();
        let phases: serde_json::Map<String, Value> = follower.phases().into_iter().map(|(part, seconds)| (part.to_string(), json!(seconds))).collect();
        println!(
            "{}",
            json!({ "row": "codex-relist-held", "measure": "a thread of 500 turns, 20,000 items, read whole in memory and committed",
                    "seconds": took.as_secs_f64(), "reading_seconds": reading.as_secs_f64(), "phases": phases, "grew_mib": grew, "written": written, "gone": gone })
        );
        Ok(true)
    })
}

pub(crate) fn kept_same(dir: &Path, _scale: Scale) -> Result<bool, String> {
    let (held, kept) = (three(&dir.join("held"))?, three(&dir.join("kept"))?);
    let accept = |_: jiff::Timestamp| true;
    let control = dir.join("grow");
    std::fs::write(&control, "150").map_err(error)?;
    runtime()?.block_on(async {
        let in_memory = Follower::new(held.producer.clone(), held.device.clone(), SCOPE)?;
        let on_disk = Follower::new(kept.producer.clone(), kept.device.clone(), SCOPE)?;
        let mut all = true;
        for (step, shape) in [("under-way", "advance"), ("relist", "relist"), ("relist-again", "relist")] {
            let server = client(&dir.join(step), shape, (shape == "advance").then_some(control.as_path())).await?;
            let read = drill::tail(&server, THREAD, &Seam::default(), &accept, None).await?.map_err(|why| format!("{step}: the reading restarted: {why}"))?;
            let mut keeping = on_disk.keeping(dir.join("spill"));
            let kept_read =
                drill::tail(&server, THREAD, &Seam::default(), &accept, Some(&mut keeping)).await?.map_err(|why| format!("{step}: the kept reading restarted: {why}"))?;
            let read_same = read.seam == kept_read.seam && read.since == kept_read.since && read.settled == kept_read.settled && read.whole && kept_read.whole && kept_read.kept && kept_read.events.is_empty();
            let events = read.events.len();
            let counts = in_memory.read(read.events, true, read.since, read.settled, read.seam)?;
            let kept_counts = on_disk.read_kept(keeping, kept_read.settled, kept_read.seam)?;
            let (items, kept_items) = (in_memory.items()?, on_disk.items()?);
            let differing = items.iter().zip(&kept_items).filter(|(one, other)| one != other).count() + items.len().abs_diff(kept_items.len());
            let checkpoint_same = in_memory.checkpoint()? == on_disk.checkpoint()?;
            let same = read_same && counts == kept_counts && differing == 0 && checkpoint_same;
            all &= same;
            println!(
                "{}",
                json!({ "row": "codex-kept-same", "step": step, "events": events, "items": items.len(), "kept_items": kept_items.len(),
                        "written_gone": counts, "kept_written_gone": kept_counts, "items_differing": differing,
                        "reading_same": read_same, "checkpoint_same": checkpoint_same, "same": same })
            );
        }
        Ok(all)
    })
}
