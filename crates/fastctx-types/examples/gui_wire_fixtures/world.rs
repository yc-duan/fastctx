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
    vec![Writer::User, Writer::Resident { address: address() }, Writer::Session { agent: "claude".parse().unwrap() }]
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
                version: i as u32 + 1,
                name: "Example project 世界".into(),
                what: "Exercise the ledger contract".into(),
                machines: vec![machine(), "laptop".parse().unwrap()],
                places: vec![Listing { what: "checkout".into(), location: location() }],
                canon: (i != 0).then(location),
                by,
                on: machine(),
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
            about: About {
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
                model: (i != 0).then(|| "model-a".into()),
            },
            town: (i != 0).then(|| "project-0".parse().unwrap()),
            here: i != 2,
            now: Some(Now {
                activity,
                since: at(),
                turn: (i == 0).then(|| "turn-1".into()),
                owed: if i == 0 { BTreeMap::from([("turn-1".into(), vec![address()])]) } else { BTreeMap::new() },
                answering: if i == 0 { vec![address()] } else { vec![] },
                trouble: (i == 2).then(|| Trouble { at: at(), text: "host disconnected".into() }),
                desk: i == 0,
                waiting_for_user: i == 0,
            }),
            last_action: (i == 0).then(|| Action {
                at: at(),
                machine: machine(),
                hand: "run".into(),
                what: "pwd".into(),
                result: "exit 0".into(),
            }),
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
            machine: machine(),
            scope,
            machines: vec![
                MachineWho::Answered {
                    reply: WhoReply {
                        machine: machine(),
                        residents: residents(),
                        jobs: [None, Some(u32::MAX)]
                            .into_iter()
                            .map(|pid| RunningJob {
                                job: "j-abc123".parse().unwrap(),
                                machine: machine(),
                                owner: address(),
                                started_at: at(),
                                pid,
                                command: "echo fixture".into(),
                                output: "C:/example/job.log".into(),
                            })
                            .collect(),
                    },
                },
                MachineWho::Offline { machine: "laptop".parse().unwrap(), since: Some(at()) },
                MachineWho::Offline { machine: "unknown".parse().unwrap(), since: None },
                MachineWho::NoAnswer { machine: "lab".parse().unwrap(), reason: "node disconnected".into() },
            ],
        })
        .collect()
}

fn nodes() -> Vec<NodesReply> {
    let mut links = vec![
        LinkState::Alone,
        LinkState::Connecting { since: at(), last_error: None },
        LinkState::Connecting { since: at(), last_error: Some("hub unavailable".into()) },
        LinkState::Stopped { reason: "machine revoked".into() },
    ];
    for route in [
        Route::Local,
        Route::Card { name: "Ethernet".into() },
        Route::Proxy { proxy: "http://proxy.example.test:8080".into() },
        Route::System,
    ] {
        links.push(LinkState::Online { since: at(), hub: "hub.example.test".into(), route });
    }
    links
        .into_iter()
        .enumerate()
        .map(|(i, link)| NodesReply {
            machine: machine(),
            world: (i != 0).then(|| "Example world".into()),
            link,
            machines: vec![
                Node {
                    name: machine(),
                    online: Some(true),
                    since: Some(at()),
                    revoked: None,
                    facts: Ok(MachineFacts {
                        name: machine(),
                        os: "windows".into(),
                        arch: "x86_64".into(),
                        home: "C:/Users/example".into(),
                        temp: "C:/Temp".into(),
                        path_separator: '\\',
                        case_sensitive: false,
                        version: "fixture-version".into(),
                    }),
                },
                Node {
                    name: "lab".parse().unwrap(),
                    online: None,
                    since: None,
                    revoked: None,
                    facts: Ok(MachineFacts {
                        name: "lab".parse().unwrap(),
                        os: "linux".into(),
                        arch: "aarch64".into(),
                        home: "/home/example".into(),
                        temp: "/tmp".into(),
                        path_separator: '/',
                        case_sensitive: true,
                        version: "fixture-version".into(),
                    }),
                },
                Node {
                    name: "laptop".parse().unwrap(),
                    online: Some(false),
                    since: Some(at()),
                    facts: Err("offline".into()),
                    revoked: Some(Revoked { by: machine(), at: at(), reason: "retired".into() }),
                },
            ],
        })
        .collect()
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
