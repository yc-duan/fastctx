use std::collections::BTreeMap;

use fastctx_types::project::*;
use fastctx_types::residents::*;
use fastctx_types::town::*;
use fastctx_types::world::*;
use fastctx_types::{Address, MachineName};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

#[derive(Serialize)]
pub struct Fixtures {
    pub who: Vec<WorldWho>,
    pub nodes: Vec<NodesReply>,
    pub towns: Vec<TownSummary>,
    pub journals: Vec<JournalSummary>,
}

fn at() -> Timestamp {
    "2026-10-04T12:06:07Z".parse().unwrap()
}

fn machine() -> MachineName {
    "desktop".parse().unwrap()
}

fn address() -> Address {
    "codex-ab12@desktop".parse().unwrap()
}

fn host() -> Host {
    Host { agent: "codex".parse().unwrap(), client: "codex-mcp-client".into(), version: "fixture-version".into() }
}

fn writers() -> Vec<Writer> {
    vec![
        Writer::User,
        Writer::Resident { address: address() },
        Writer::Session { agent: "claude".parse().unwrap() },
        // As told to whoever may not work on the machine it was written on.
        Writer::Elsewhere { page: false },
        Writer::Elsewhere { page: true },
    ]
}

fn location() -> Location {
    Location { machine: machine(), path: "C:/work/project".into() }
}

fn towns() -> Vec<TownSummary> {
    writers()
        .into_iter()
        .enumerate()
        .map(|(i, by)| TownSummary {
            town: format!("project-{i}").parse().unwrap(),
            record: Record {
                on: (!matches!(by, Writer::Elsewhere { .. })).then(machine),
                version: i as u32 + 1,
                name: "Example project 世界".into(),
                what: "Exercise the ledger contract".into(),
                machines: vec![machine(), "laptop".parse().unwrap()],
                places: vec![Listing { what: "checkout".into(), location: location() }],
                canon: (i != 0).then(location),
                by,
                at: at(),
            },
            journals: i,
        })
        .collect()
}

fn residents() -> Vec<ResidentNow> {
    let mut residents: Vec<_> = [Activity::Busy, Activity::Idle, Activity::Asleep]
        .into_iter()
        .enumerate()
        .map(|(i, activity)| ResidentNow {
            source: None,
            tools: vec![CurrentTool {
                turn: "turn-1".into(),
                id: "call-1".into(),
                started_at: Some(at()),
                name: "run".into(),
                machine: Some(machine()),
                detail: "cargo test".into(),
            }],
            about: About {
                side: false,
                forked_from: None,
                mcp_off: Default::default(),
                id: format!("codex-ab1{i}").parse().unwrap(),
                host: host(),
                machine: machine(),
                born_at: at(),
                origin: if i == 0 {
                    Origin::User
                } else {
                    Origin::Colleague { asked_by: address(), label: "Check JSON".into() }
                },
                session: (i != 0).then(|| "thread-1".into()),
                world_started: i != 0,
                native_resumed: false,
                model: (i != 0).then(|| "model-a".into()),
                acting: if i == 0 {
                    fastctx_types::access::Acting::Here
                } else {
                    fastctx_types::access::Acting::one("w-0123456789abcdef".parse().unwrap(), "ada".parse().unwrap())
                },
                focus: if i == 1 {
                    fastctx_types::focus::Focus {
                        machines: vec![fastctx_types::focus::FocusedMachine {
                            machine: "laptop.home".parse().unwrap(),
                            folders: vec!["/home/ada/app".into()],
                        }],
                    }
                } else {
                    Default::default()
                },
                home: (i == 1).then(|| "C:/Users/ada/.codex-work".into()),
                callee: None,
            },
            town: (i != 0).then(|| "project-0".parse().unwrap()),
            here: i != 2,
            now: Some(Now {
                activity,
                since: at(),
                turn: (i == 0).then(|| "turn-1".into()),
                last_turn: None,
                owed: if i == 0 { BTreeMap::from([("turn-1".into(), vec![address()])]) } else { BTreeMap::new() },
                answering: if i == 0 { vec![address()] } else { vec![] },
                trouble: (i == 2).then(|| Trouble { at: at(), text: "host disconnected".into() }),
                desk: i == 0,
                desk_since: Some(at()),
                waiting_for_user: i == 0,
                waiting_since: Some(at()),
                title: (i == 0).then(|| "端口对拍".to_string()),
                last_tool_at: Some(at()),
                first_words: (i == 1).then(|| "把端口对拍一遍".to_string()),
                last_said: (i == 1).then(|| "两边一致，共 12 个端口。".to_string()),
                last_compacted: i == 2,
                last_heard_user: (i == 1).then_some(true),
                asked: match i {
                    0 => Some(fastctx_types::residents::Asked {
                        id: r#"["codex","r-1"]"#.into(),
                        at: at(),
                        waits: true,
                        what: fastctx_types::residents::AskedFor::Command,
                        text: "cargo test".into(),
                        options: vec![],
                    }),
                    1 => Some(fastctx_types::residents::Asked {
                        id: "msg-7".into(),
                        at: at(),
                        waits: false,
                        what: fastctx_types::residents::AskedFor::Answer,
                        text: "要重新允许 computer-use 吗？".into(),
                        options: vec!["允许".into(), "不用".into()],
                    }),
                    _ => None,
                },
                told: (i == 1).then(|| fastctx_types::residents::Told {
                    id: "told-1".into(),
                    at: at(),
                    status: fastctx_types::residents::TellStatus::Urgent,
                    text: "训练在 11,200 步之后发散了。".into(),
                    turn: Some("turn-3".into()),
                }),
            }),
            last_action: (i == 0).then(|| Action {
                turn: None,
                at: at(),
                machine: machine(),
                hand: "run".into(),
                what: "pwd".into(),
                result: "exit 0".into(),
            }),
            removed: None,
            moved: match i {
                0 => None,
                1 => Some(Moved { to: Some("project-2".parse().unwrap()), at: at() }),
                _ => Some(Moved { to: None, at: at() }),
            },
        })
        .collect();
    // Old persisted data can omit defaults; today's reply emits every field.
    let about: About = serde_json::from_value(json!({
        "id": "claude-ab12", "host": host(), "machine": machine(), "born_at": at(),
        "origin": {"kind": "user"}, "session": null,
    }))
    .unwrap();
    residents.push(
        serde_json::from_value(json!({"about": about, "town": null, "here": false, "now": null, "last_action": null}))
            .unwrap(),
    );
    let now: Now = serde_json::from_value(
        json!({"activity": "idle", "since": at(), "turn": null, "answering": [], "trouble": null}),
    )
    .unwrap();
    let mut legacy = residents[3].clone();
    legacy.now = Some(now);
    residents.push(legacy);
    let mut removed = residents[0].clone();
    removed.here = false;
    removed.removed = Some(at());
    residents.push(removed);
    residents
}

fn who(towns: &[TownSummary]) -> Vec<WorldWho> {
    let scopes = vec![
        WhoScope::World { towns: towns.to_vec() },
        WhoScope::Town { town: "project-0".parse().unwrap(), record: Some(Box::new(towns[0].record.clone())) },
        WhoScope::Town { town: "project-unknown".parse().unwrap(), record: None },
    ];
    scopes
        .into_iter()
        .map(|scope| WorldWho {
            history: if matches!(&scope, WhoScope::World { .. }) {
                super::since::fixtures().replies.into_iter().find_map(|reply| match reply {
                    fastctx_types::Reply::Since(history) => Some(history),
                    _ => None,
                })
            } else {
                None
            },
            you: Some(address()),
            machine: machine(),
            scope,
            machines: vec![
                MachineWho::Answered {
                    observation: None,
                    reply: WhoReply {
                        history: None,
                        busy: Some(2),
                        machine: machine(),
                        residents: residents(),
                        jobs: [None, Some(u32::MAX)]
                            .into_iter()
                            .map(|pid| RunningJob {
                                source: None,
                                town: None,
                                job: "j-abc123".parse().unwrap(),
                                machine: machine(),
                                owner: address().into(),
                                started_at: at(),
                                pid,
                                command: "echo fixture".into(),
                                output: "C:/example/job.log".into(),
                                stderr: None,
                                account: pid.map(|_| fastctx_types::access::HubAccount {
                                    hub: "w-0123456789abcdef".parse().unwrap(),
                                    account: "bob".parse().unwrap(),
                                }),
                            })
                            .collect(),
                    },
                },
                MachineWho::Offline {
                    observation: None,
                    machine: "laptop".parse().unwrap(),
                    since: Some(at()),
                    last: Some(Box::new(WhoReply {
                        history: None,
                        busy: None,
                        machine: "laptop".parse().unwrap(),
                        residents: residents().into_iter().take(1).collect(),
                        jobs: Vec::new(),
                    })),
                },
                MachineWho::Offline { observation: None, machine: "unknown".parse().unwrap(), since: None, last: None },
                MachineWho::NoAnswer {
                    observation: None,
                    machine: "lab".parse().unwrap(),
                    reason: "node disconnected".into(),
                },
            ],
            others: vec![fastctx_types::town::WhoFor {
                source: None,
                machine: machine(),
                id: "j-abc123".into(),
                accounts: vec!["bob".parse().unwrap()],
            }],
            involved: Vec::new(),
        })
        .collect()
}

fn nodes() -> Vec<NodesReply> {
    let mut links = vec![
        LinkState::Alone,
        LinkState::Connecting { since: at(), last_error: None },
        LinkState::Connecting { since: at(), last_error: Some("hub unavailable".into()) },
        LinkState::Stopped { reason: "machine revoked".into(), dissolved: false },
        LinkState::Stopped { reason: "the network was dissolved".into(), dissolved: true },
    ];
    for route in [
        Route::Local,
        Route::Card { name: "Ethernet".into() },
        Route::Proxy { proxy: "http://proxy.example.test:8080".into() },
        Route::System,
    ] {
        links.push(LinkState::Online { since: at(), hub: "hub.example.test".into(), through: None, route, ms: None });
    }
    links.push(LinkState::Online {
        since: at(),
        hub: "203.0.113.5:7444".into(),
        through: Some("server".parse().unwrap()),
        route: Route::System,
        ms: Some(41),
    });
    let facts = |name: &str, os: &str, lifetime: bool| MachineFacts {
        turn_debts: false,
        goal_runtime: false,
        goal_colleagues: lifetime,
        goal_waits: lifetime,
        goal_questions: lifetime,
        goal_question_words: lifetime,
        goal_commands: lifetime,
        goal_reports: lifetime,
        sentinels: false,
        sentinel_look: false,
        name: name.parse().unwrap(),
        os: os.into(),
        arch: if os == "windows" { "x86_64".into() } else { "aarch64".into() },
        home: if os == "windows" { "C:/Users/example".into() } else { "/home/example".into() },
        temp: if os == "windows" { "C:/Temp".into() } else { "/tmp".into() },
        path_separator: if os == "windows" { '\\' } else { '/' },
        case_sensitive: os != "windows",
        version: "fixture-version".into(),
        mcp: true,
        mcp_import: true,
        page_accounts: true,
        attachment_places: true,
        attachment_runs: true,
        memory_folders: false,
        collaboration_tree: false,
        completion_receipts: false,
        control_letters: lifetime,
        run_stops: lifetime,
        upload_resumes: false,
        attachment_references: false,
        stats: lifetime,
        shell: String::new(),
        form: None,
        node_lifetime: lifetime.then(|| {
            Box::new(fastctx_types::node_lifetime::NodeLifetime {
                process: fastctx_types::ProcessIdentity { pid: 123, token: "node-created".into() },
                starter: Some(fastctx_types::node_lifetime::NodeStarter {
                    process: fastctx_types::ProcessIdentity { pid: 122, token: "serve-created".into() },
                    session: Some(Host {
                        agent: "codex".parse().unwrap(),
                        client: "codex-fixture".into(),
                        version: "fixture-version".into(),
                    }),
                }),
                supervised: false,
                windows_job: Some(fastctx_types::node_lifetime::WindowsJob {
                    kill_on_close: true,
                    breakaway_allowed: false,
                    silent_breakaway: false,
                }),
            })
        }),
    };
    let me: fastctx_types::AccountName = "ada".parse().unwrap();
    let all = fastctx_types::access::Rights { hands: true, agents: true };
    let agents = fastctx_types::access::Rights { hands: false, agents: true };
    let mut replies: Vec<NodesReply> = links
        .into_iter()
        .enumerate()
        .map(|(i, link)| NodesReply {
            machine: machine(),
            facts: Some(facts("desktop", "windows", true)),
            hubs: vec![HubNodes {
                hub: "w-0123456789abcdef".parse().unwrap(),
                name: "home".parse().unwrap(),
                link,
                logins: vec![me.clone()],
                home: Some(me.clone()),
                here: vec![(me.clone(), all)],
                machines: vec![
                    Node {
                        name: "lab".parse().unwrap(),
                        written: if i == 1 { "lab.home".into() } else { "lab".into() },
                        online: None,
                        since: None,
                        through: Some("server".into()),
                        allows_direct: false,
                        path: Some(fastctx_types::world::Path::Hub { why: None }),
                        facts: Ok(facts("lab", "linux", false)),
                        owner: Some("grace".parse().unwrap()),
                        group: None,
                        rights: vec![(me.clone(), agents)],
                    },
                    Node {
                        name: "laptop".parse().unwrap(),
                        written: "laptop".into(),
                        online: Some(false),
                        since: Some(at()),
                        through: None,
                        allows_direct: false,
                        path: None,
                        facts: Err("offline".into()),
                        owner: Some(me.clone()),
                        group: Some("lab A".into()),
                        rights: vec![(me.clone(), all)],
                    },
                    Node {
                        name: "server".parse().unwrap(),
                        written: "server".into(),
                        online: Some(true),
                        since: Some(at()),
                        through: None,
                        allows_direct: true,
                        path: Some(if i == 2 {
                            fastctx_types::world::Path::Hub {
                                why: Some("203.0.113.20:7444: no answer within 30 s".into()),
                            }
                        } else {
                            fastctx_types::world::Path::Direct { ms: Some(41) }
                        }),
                        facts: Ok(facts("server", "linux", false)),
                        owner: Some(me.clone()),
                        group: None,
                        rights: vec![(me.clone(), all)],
                    },
                ],
            }],
        })
        .collect();
    replies.push(NodesReply { machine: machine(), facts: Some(facts("desktop", "windows", false)), hubs: Vec::new() });
    replies
}

fn journals() -> Vec<JournalSummary> {
    let mut journals = vec![JournalSummary {
        journal: "empty".parse().unwrap(),
        description: None,
        entries: 0,
        writers: 0,
        latest: None,
    }];
    for (i, by) in writers().into_iter().enumerate() {
        journals.push(JournalSummary {
            journal: format!("wire-{i}").parse().unwrap(),
            description: Some(Description {
                version: 1,
                text: "Wire shapes".into(),
                by: by.clone(),
                on: machine(),
                at: at(),
            }),
            entries: 2,
            writers: 1,
            latest: Some(EntryStamp {
                id: "20261004T120607Z-desktop".parse().unwrap(),
                at: at(),
                machine: machine(),
                by,
                agent: (i != 0).then(host),
                model: (i != 0).then(|| Model { name: "model-a".into(), self_reported: i == 2 }),
                session: (i != 0).then(|| "thread-1".into()),
                cites: vec!["20261003T120607Z-laptop".parse().unwrap()],
                attachments: [AttachmentKind::Text, AttachmentKind::Image, AttachmentKind::Pdf, AttachmentKind::Binary]
                    .into_iter()
                    .enumerate()
                    .map(|(i, kind)| Attachment {
                        name: format!("attachment-{i}"),
                        kind,
                        bytes: if i == 0 { 0 } else { u64::MAX },
                        sha256: "0".repeat(64),
                        kept: if i % 2 == 0 { Kept::InEntry } else { Kept::OnMachine { location: location() } },
                    })
                    .collect(),
            }),
        });
    }
    journals
}

pub fn fixtures() -> Fixtures {
    let towns = towns();
    Fixtures { who: who(&towns), nodes: nodes(), towns, journals: journals() }
}
