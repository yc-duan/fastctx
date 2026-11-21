use super::*;
use crate::timeline::assets::tests::{Device, all, library};
use fastctx_hosts::codex::drill::{self, AppServer, Seam as HostSeam, Tail, tungstenite};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path};

const FIRST_ITEMS: usize = 2_100;
const OLD: &str = "previous";
const TURN: &str = "long-turn";
const NEXT: &str = "after-revert";
const THREAD: &str = "thread";
const FIRST_TIME: i64 = 1_800_000_000_000;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

fn when(step: usize) -> i64 {
    FIRST_TIME + (FIRST_ITEMS as i64 + (step as i64 * 5) + 1) * 20
}

fn changed(turn: Option<&str>, ms: i64, change: SessionChange) -> SessionEvent {
    SessionEvent { at: at(ms), happened: Happened::Changed { turn: turn.map(str::to_owned), change } }
}

fn start(turn: &str, ms: i64) -> SessionEvent {
    SessionEvent { at: at(ms), happened: Happened::TurnStarted { turn: turn.into() } }
}

fn put(timeline: &mut Vec<Value>, items: &mut Vec<Value>, turn: &str, position: usize, ms: i64, item: Value) {
    timeline.push(json!({"type":"item","position":position,"turnId":turn,"item":item}));
    items.push(json!({"turnId":turn,"item":item,"startedAtMs":ms-3,"completedAtMs":ms}));
}

fn row(n: usize) -> Value {
    json!({"id":format!("initial-{n}"),"type":"agentMessage","text":format!("stored item {n}")})
}

fn fixture(step: usize) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let mut timeline = vec![json!({"type":"turnStarted","position":1,"turnId":OLD,"startedAt":FIRST_TIME/1000-3})];
    let mut items = Vec::new();
    put(
        &mut timeline,
        &mut items,
        OLD,
        2,
        FIRST_TIME - 2_900,
        json!({"id":"previous-item","type":"agentMessage","text":"before"}),
    );
    timeline.push(
        json!({"type":"turnCompleted","position":3,"turnId":OLD,"status":"completed","completedAt":FIRST_TIME/1000-2}),
    );
    timeline.push(json!({"type":"turnStarted","position":4,"turnId":TURN,"startedAt":FIRST_TIME/1000}));
    let initial = if step >= 132 { FIRST_ITEMS - 12 } else { FIRST_ITEMS };
    for n in 0..initial {
        put(&mut timeline, &mut items, TURN, 5 + n, FIRST_TIME + 20 * (n as i64 + 1), row(n));
    }
    let mut position = 5 + initial;
    if step < 132 {
        for n in 1..=step.min(129) {
            let ms = when(n);
            let text = if n == 59 && step >= 60 {
                "corrected previous completion".to_string()
            } else if n == 45 && step >= 95 {
                "corrected older completion".to_string()
            } else {
                format!("new item {n}")
            };
            let id = format!("append-{n}");
            if n == 69 {
                put(
                    &mut timeline,
                    &mut items,
                    TURN,
                    position,
                    ms - 12,
                    json!({"type":"userMessage","id":"steer","content":[{"type":"text","text":"A steer"}]}),
                );
                position += 1;
            }
            if n == 87 {
                put(
                    &mut timeline,
                    &mut items,
                    OLD,
                    position,
                    ms - 8,
                    json!({"type":"agentMessage","id":"late-old","text":"late item of previous turn"}),
                );
                position += 1;
            }
            if n % 7 == 0 {
                put(
                    &mut timeline,
                    &mut items,
                    TURN,
                    position,
                    ms - 6,
                    json!({"type":"commandExecution","id":format!("delayed-{}",n-1),
                    "command":"printf fake","source":"unifiedExecStartup","status":"completed","aggregatedOutput":"ok","exitCode":0,"durationMs":25}),
                );
                items.last_mut().unwrap()["startedAtMs"] = json!(when(n - 1) - 9);
                position += 1;
            }
            if n % 8 == 0 {
                put(
                    &mut timeline,
                    &mut items,
                    TURN,
                    position,
                    ms - 4,
                    json!({"id":format!("extra-{n}"),"type":"agentMessage","text":"extra"}),
                );
                position += 1;
            }
            put(&mut timeline, &mut items, TURN, position, ms - 1, json!({"id":id,"type":"agentMessage","text":text}));
            position += 1;
        }
        if step >= 130 {
            // The final item and a boundary share an ordinal and are served on different pages.
            timeline.push(json!({"type":"turnCompleted","position":position-1,"turnId":TURN,"status":"completed","completedAt":when(130)/1000}));
        }
    } else {
        timeline.push(json!({"type":"turnCompleted","position":position,"turnId":TURN,"status":"interrupted","completedAt":when(131)/1000}));
        position += 1;
        timeline.push(json!({"type":"turnStarted","position":position,"turnId":NEXT,"startedAt":when(132)/1000}));
        position += 1;
        for n in 132..=step {
            put(
                &mut timeline,
                &mut items,
                NEXT,
                position,
                when(n) + 2,
                json!({"id":format!("next-{n}"),"type":"agentMessage","text":"after revert"}),
            );
            position += 1;
        }
    }
    let turn = |id, status, start| json!({"id":id,"status":status,"startedAt":start});
    let mut turns = vec![
        turn(OLD, "completed", FIRST_TIME / 1000 - 3),
        turn(TURN, if step < 130 { "inProgress" } else { "completed" }, FIRST_TIME / 1000),
    ];
    if step >= 132 {
        turns.push(turn(NEXT, "inProgress", when(132) / 1000));
    }
    (timeline, items, turns)
}

fn fake_answer(
    step: usize,
    method: &str,
    params: &Value,
    fixture: &(Vec<Value>, Vec<Value>, Vec<Value>),
) -> Result<Value, (i32, &'static str)> {
    if method == "initialize" {
        return Ok(json!({"userAgent":"codex_cli_rs/0.160.0 (test)"}));
    }
    if method == "thread/read" {
        let path = if step < 54 {
            "source"
        } else if step < 132 {
            "renamed"
        } else {
            "reverted"
        };
        return Ok(json!({"thread":{"id":THREAD,"historyMode":"paginated","path":path,"name":"Codex probe",
           "updatedAt":when(step)/1000,"createdAt":FIRST_TIME/1000-3}}));
    }
    let (timeline, items, turns) = fixture;
    let desc = params["sortDirection"] == "desc" || method == "thread/timeline/list";
    let cursor = params["cursor"].as_str().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
    let cap = if method == "thread/timeline/list" && step == 130 { 1 } else { 100 };
    let limit = params["limit"].as_u64().unwrap_or(25).clamp(1, cap) as usize;
    let (mut entries, from) = match method {
        "thread/timeline/list" => (timeline.iter().collect::<Vec<_>>(), cursor),
        "thread/turns/list" => (turns.iter().collect::<Vec<_>>(), cursor),
        "thread/items/list" => {
            let rows: Vec<_> =
                items.iter().filter(|row| params["turnId"].is_null() || row["turnId"] == params["turnId"]).collect();
            let from = if let Some(item) = params.pointer("/cursor/itemId").and_then(Value::as_str) {
                if desc || params["turnId"].is_null() {
                    return Err((-32602, "bad anchor scope"));
                }
                rows.iter().position(|row| row["item"]["id"] == item).ok_or((-32602, "unknown item anchor"))? + 1
            } else {
                cursor
            };
            (rows, from)
        }
        _ => return Err((-32601, "unknown method")),
    };
    if desc {
        entries.reverse();
    }
    let through = (from + limit).min(entries.len());
    let next = (through < entries.len()).then(|| through.to_string());
    let mut page = entries[from.min(entries.len())..through].to_vec();
    if method == "thread/timeline/list" {
        page.reverse();
    }
    Ok(json!({"data":page,"nextCursor":next,"activeRealtimeSessionAtPageStart":null}))
}

#[test]
#[ignore = "used as the mock app-server child"]
fn fake_codex_child() {
    let socket = std::env::var("MOCK_CODEX_SOCKET").unwrap();
    let listener = std::os::unix::net::UnixListener::bind(socket.strip_prefix("unix://").unwrap()).unwrap();
    eprintln!(
        r#"{{"target":"codex_app_server_transport::transport::unix_socket","fields":{{"message":"app-server control socket listening"}}}}"#
    );
    let (stream, _) = listener.accept().unwrap();
    let mut socket = tungstenite::accept(stream).unwrap();
    let control = std::env::var("MOCK_CODEX_CONTROL").unwrap();
    let (mut previous, mut snapshot) = (0, fixture(0));
    loop {
        let raw = match socket.read() {
            Ok(tungstenite::Message::Text(text)) => text,
            Ok(tungstenite::Message::Close(_)) | Err(_) => break,
            _ => continue,
        };
        let request: Value = serde_json::from_str(&raw).unwrap();
        if request["id"].is_null() {
            continue;
        }
        let step = fastctx_platform::disk::read_to_string(&control).unwrap().parse().unwrap();
        let method = request["method"].as_str().unwrap();
        if step != previous {
            snapshot = fixture(step);
            previous = step;
        }
        let response = match fake_answer(step, method, &request["params"], &snapshot) {
            Ok(result) => json!({"id":request["id"],"result":result}),
            Err((code, message)) => json!({"id":request["id"],"error":{"code":code,"message":message}}),
        };
        if socket.send(tungstenite::Message::Text(response.to_string().into())).is_err() {
            break;
        }
    }
}

fn appserver(dir: &Path, control: &Path) -> Arc<AppServer> {
    let launcher = dir.join("app-server-mock");
    fastctx_platform::disk::write(
        &launcher,
        r#"#!/bin/sh
export MOCK_CODEX_SOCKET="$2"
exec "$MOCK_CODEX_TEST" "$MOCK_CODEX_FN" --exact --ignored --nocapture
"#,
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    let fn_path =
        format!("{}::fake_codex_child", module_path!().strip_prefix("fastctx_node::").unwrap_or(module_path!()));
    let names = [
        ("MOCK_CODEX_TEST".into(), std::env::current_exe().unwrap().into_os_string()),
        ("MOCK_CODEX_FN".into(), fn_path.into()),
        ("MOCK_CODEX_CONTROL".into(), control.as_os_str().to_owned()),
    ];
    let home = dir.join("home");
    fastctx_platform::disk::create_dir_all(&home).unwrap();
    drill::spawn(&launcher, &[], &home, &names, Arc::new(|line| eprintln!("{line}"))).unwrap()
}

struct Track {
    library: Library,
    placed: assets::Placed,
    seam: HostSeam,
    floor: u64,
    held: Option<u64>,
    holding: Option<i64>,
    told: Vec<String>,
    replaced: usize,
}

impl Track {
    fn new(path: &Path) -> Self {
        let (library, placed) = library(path);
        Self {
            library,
            placed,
            seam: HostSeam::default(),
            floor: 0,
            held: None,
            holding: None,
            told: Vec::new(),
            replaced: 0,
        }
    }

    async fn judge(
        &mut self,
        server: &AppServer,
        resident: &ResidentId,
        running: Option<&str>,
        world: &[SessionEvent],
    ) -> (Tail, Vec<SessionEvent>, Vec<SessionEvent>) {
        if world[self.replaced..].iter().any(talk::changes_history) {
            self.seam = HostSeam::default();
        }
        self.replaced = world.len();
        let tail = match drill::tail_running(server, THREAD, running, &self.seam).await.unwrap() {
            Ok(tail) => tail,
            Err(_) => drill::tail_running(server, THREAD, running, &HostSeam::default()).await.unwrap().unwrap(),
        };
        let since = tail.since.unwrap_or(Timestamp::MIN);
        let settled = tail.settled.unwrap_or(since);
        let filtered: Vec<_> = world.iter().filter(|e| tail.whole || e.at >= since).cloned().collect();
        let filtered = crate::codex::watch::recorded(filtered, &tail.events);
        let history = talk::merged(filtered.clone(), Some(tail.events.clone()), "codex", &NoReceipts);
        let told: Vec<_> = tail.events.iter().filter(|e| told_again(e)).cloned().collect();
        let name = assets::checkpoint(resident);
        let generation = self.held.map_or(1, |old| old + 1);
        let stood = Stood {
            name: name.clone(),
            held: self.held,
            told: self.told.clone(),
            floor: self.floor,
            holding: self.holding,
            generation,
        };
        let host_told = told.iter().map(|event| serde_json::to_value(event).unwrap()).collect();
        let input = Reading { history, host_told, whole: tail.whole, since, settled, codex: true };
        let got = place(&self.library, Arc::new(Device), resident, &self.placed, &stood, input).unwrap();
        self.seam = tail.seam.clone();
        self.floor = got.floor;
        self.held = Some(got.held);
        self.holding = Some(got.wrote.holding);
        self.told = got.told;
        self.library.sweep_held(&name, self.held).unwrap();
        (tail, filtered, told)
    }
}

fn stepped(world: &mut Vec<SessionEvent>, step: usize) {
    match step {
        0 => world.push(start(TURN, FIRST_TIME)),
        17 => world.push(changed(
            Some(TURN),
            when(step),
            SessionChange::Request { id: json!(1), request: "approval".into(), params: Value::Null },
        )),
        18 => world.push(changed(
            Some(TURN),
            when(step),
            SessionChange::Resolved { id: json!(1), resolution: Value::Null },
        )),
        69 => world.push(SessionEvent {
            at: at(when(step) - 11),
            happened: Happened::Heard {
                delivery: None,
                turn: Some(TURN.into()),
                from: None,
                text: "A steer".into(),
                message: None,
                notice: None,
            },
        }),
        130 => world.push(SessionEvent {
            at: at(when(step)),
            happened: Happened::TurnEnded {
                turn: TURN.into(),
                end: serde_json::from_value(json!({"how":"completed","reason":null,"duration_ms":null})).unwrap(),
            },
        }),
        132 => {
            world.push(changed(None, when(131), SessionChange::HistoryChanged));
            world.push(start(NEXT, when(132)));
        }
        n if n < 130 && n % 5 == 0 => {
            world.push(SessionEvent {
                at: at(when(n)),
                happened: Happened::Context { used: n as u64, window: Some(200_000) },
            });
            world.push(changed(Some(TURN), when(n), SessionChange::Usage { usage: json!({"tokens":n}) }));
        }
        _ => {}
    }
}

#[test]
fn a_codex_window_written_on_equals_the_judge_after_every_look() {
    let dir = tempfile::tempdir().unwrap();
    let control = dir.path().join("step");
    fastctx_platform::disk::write(&control, "0").unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let server = appserver(dir.path(), &control);
        let _ = server.request("initialize", json!({})).await.unwrap();
        let resident: ResidentId = "codex-a0a0".parse().unwrap();
        let checkpoint = assets::checkpoint(&resident);
        let (mut judge, mut followed) =
            (Track::new(&dir.path().join("judge")), Track::new(&dir.path().join("followed")));
        let mut world = Vec::new();
        let mut window: Option<Window> = None;
        let mut refusals = BTreeMap::<String, usize>::new();
        let mut fast = 0;
        for step in 0..141 {
            fastctx_platform::disk::write(&control, step.to_string()).unwrap();
            let old = world.len();
            stepped(&mut world, step);
            let running = if step < 130 {
                Some(TURN)
            } else if step >= 132 {
                Some(NEXT)
            } else {
                None
            };
            let result = if let Some(w) = window.as_mut() {
                match drill::onward(&server, THREAD, running, w.through()).await.unwrap() {
                    // The host has no public revision marker for an old item.
                    Ok(_) if step == 95 => Err("periodic audit of a silent older revision"),
                    Ok(delta) => w
                        .write_on(
                            &followed.library,
                            Arc::new(Device),
                            &resident,
                            &followed.placed,
                            None,
                            &checkpoint,
                            followed.held.unwrap(),
                            followed.holding,
                            delta,
                            &world[old..],
                        )
                        .unwrap(),
                    Err(why) => Err(why),
                }
            } else {
                Err("no window")
            };
            match result {
                Ok((wrote, told)) => {
                    followed.holding = Some(wrote.holding);
                    followed.told = told;
                    fast += 1;
                }
                Err(why) => {
                    *refusals.entry(why.into()).or_default() += 1;
                    let (tail, world, told) = followed.judge(&server, &resident, running, &world).await;
                    window = match tail.through {
                        Some(through) => {
                            Window::seed(&followed.library, &followed.placed.scope, None, through, &world, &told)
                                .unwrap()
                                .ok()
                        }
                        None => None,
                    };
                }
            }
            let (reference, _, _) = judge.judge(&server, &resident, running, &world).await;
            if step == 130 {
                let end = reference.events.iter().find_map(|event| match &event.happened {
                    Happened::TurnEnded { turn, end } if turn == TURN => end.last_message.as_deref(),
                    _ => None,
                });
                assert_eq!(end, Some("new item 129"), "equal ordinals across pages keep item before end");
            }
            let got = all(&followed.library);
            let want = all(&judge.library);
            let difference = got.iter().zip(&want).position(|(a, b)| a != b);
            assert!(
                difference.is_none() && got.len() == want.len(),
                "look {step} differed at {difference:?}, got {} expected {}",
                got.len(),
                want.len()
            );
        }
        assert!(fast >= 120, "fast={fast} refusals={refusals:?}");
        assert!(refusals.contains_key("words steered into the running turn"));
        assert!(refusals.contains_key("a timeline row crosses the running turn"));
        assert!(refusals.contains_key("the timeline anchor changed"));
        assert!(refusals.contains_key("the rollout changed"));
        assert!(refusals.contains_key("periodic audit of a silent older revision"));
        println!("141 looks: {fast} onward, {} judge: {refusals:?}", 141 - fast);
        server.kill();
    });
}
