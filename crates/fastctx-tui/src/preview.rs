//! Scenes drawn from made-up data, to look at the pages without a World:
//! three machines, two projects, a colleague at work, a conversation in the
//! middle of a turn, a journal with entries that answer each other.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fastctx_client::TownView;
use fastctx_types::project::{
    Attachment, AttachmentKind, CanonHead, Description, EntryStamp, EntryView, JournalSummary, JournalView, Kept,
    Listing, Location, MapView, Model, Record, Stamp, TownSummary, Writer,
};
use fastctx_types::residents::{About, Action, Activity, Host, Now, Origin, Trouble};
use fastctx_types::session::{
    Change, ChangeHow, Does, Happened, Health, HealthKind, Item, Part, SessionEvent, Speaker, Step, StepStatus, Tool,
    ToolStatus, TurnEnd, TurnHow, What,
};
use fastctx_types::town::{MachineWho, ResidentNow, RunningJob, WhoReply, WhoScope, WorldWho};
use fastctx_types::world::{LinkState, MachineFacts, Node, NodesReply, Route};
use fastctx_types::{Address, AgentName, EntryId, JobId, JournalName, MachineName, ResidentId, TownName};
use jiff::{SignedDuration, Timestamp};

use crate::app::{Ctx, Data};
use crate::pages::Page;
use crate::pages::form::FormPage;
use crate::pages::journal::JournalPage;
use crate::pages::machines::MachinesPage;
use crate::pages::project::ProjectPage;
use crate::pages::talk::TalkPage;
use crate::pages::world::WorldPage;

fn machine(name: &str) -> MachineName {
    MachineName::try_from(name).expect("a machine name")
}

fn town(name: &str) -> TownName {
    TownName::try_from(name).expect("a town name")
}

fn id(name: &str) -> ResidentId {
    ResidentId::try_from(name).expect("a resident id")
}

fn address(resident: &str, on: &str) -> Address {
    Address { machine: machine(on), resident: id(resident) }
}

fn ago(now: Timestamp, minutes: i64) -> Timestamp {
    now - SignedDuration::from_mins(minutes)
}

#[allow(clippy::too_many_arguments)]
fn resident(
    now: Timestamp,
    who: &str,
    agent: &str,
    on: &str,
    origin: Origin,
    model: &str,
    in_town: Option<&str>,
    here_for: i64,
) -> ResidentNow {
    ResidentNow {
        source: None,
        about: About {
            side: false,
            forked_from: None,
            id: id(who),
            host: Host {
                agent: AgentName::try_from(agent).expect("an agent"),
                client: String::new(),
                version: String::new(),
            },
            machine: machine(on),
            born_at: ago(now, here_for),
            origin,
            session: None,
            world_started: true,
            native_resumed: false,
            model: Some(model.to_string()),
            acting: Default::default(),
            focus: Default::default(),
            home: None,
            callee: None,
        },
        town: in_town.map(town),
        here: true,
        now: None,
        last_action: None,
        tools: Vec::new(),
        moved: None,
        removed: None,
    }
}

fn state(now: Timestamp, activity: Activity, since: i64) -> Now {
    Now {
        activity,
        since: ago(now, since),
        turn: None,
        owed: BTreeMap::new(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
        first_words: None,
        last_said: None,
        asked: None,
    }
}

fn action(now: Timestamp, minutes: i64, on: &str, hand: &str, what: &str, result: &str) -> Action {
    Action {
        turn: None,
        at: ago(now, minutes),
        machine: machine(on),
        hand: hand.to_string(),
        what: what.to_string(),
        result: result.to_string(),
    }
}

fn record(now: Timestamp, name: &str, what: &str, machines: &[&str], version: u32) -> Record {
    Record {
        version,
        name: name.to_string(),
        what: what.to_string(),
        machines: machines.iter().map(|name| machine(name)).collect(),
        places: vec![
            Listing {
                what: "固件源码（git 仓库）".into(),
                location: Location { machine: machine("desktop"), path: "D:/work/mark50".into() },
            },
            Listing {
                what: "仿真与训练数据".into(),
                location: Location { machine: machine("laptop"), path: "/data/mark50-sim".into() },
            },
            Listing {
                what: "持续集成".into(),
                location: Location { machine: machine("server"), path: "/srv/mark50-ci".into() },
            },
        ],
        canon: Some(Location { machine: machine("desktop"), path: "D:/work/mark50/specs".into() }),
        by: Writer::User,
        on: machine("desktop"),
        at: ago(now, 60 * 24 * 2),
    }
}

/// The people of the World, as one ask of it would find them.
fn residents(now: Timestamp) -> (Vec<ResidentNow>, Vec<RunningJob>) {
    let mut claude =
        resident(now, "claude-2b1c", "claude", "desktop", Origin::User, "claude-opus-5-5", Some("mark50"), 130);
    claude.now = Some(state(now, Activity::Busy, 4));
    claude.last_action = Some(action(now, 1, "desktop", "write", "D:/work/mark50/specs/control-loop.md", "updated"));

    let mut codex = resident(
        now,
        "codex-7f3a",
        "codex",
        "laptop",
        Origin::Colleague {
            asked_by: address("claude-2b1c", "desktop"),
            label: "port the PID loop to fixed-point".into(),
        },
        "gpt-6.1-sol",
        Some("mark50"),
        40,
    );
    codex.now = Some(state(now, Activity::Busy, 14));
    codex.last_action = Some(action(now, 3, "laptop", "run", "cargo test --workspace", "running as j-k3x9a2"));

    let mut finder = resident(
        now,
        "codex-91c0",
        "codex",
        "desktop",
        Origin::Colleague {
            asked_by: address("claude-2b1c", "desktop"),
            label: "find every caller of encode_frame".into(),
        },
        "gpt-6.1-sol",
        Some("mark50"),
        12,
    );
    finder.now = Some(state(now, Activity::Idle, 2));
    finder.last_action = Some(action(now, 2, "desktop", "run", "rg -n 'encode_frame' D:/work/mark50/src", "exit 0"));

    let mut outside = resident(now, "codex-5e12", "codex", "desktop", Origin::User, "gpt-6.1-sol", None, 25);
    outside.now = Some(state(now, Activity::Idle, 9));
    outside.now.as_mut().expect("a state").trouble =
        Some(Trouble { at: ago(now, 9), text: "turn failed: the model's provider stopped the request".into() });
    outside.last_action = Some(action(now, 10, "desktop", "view", "C:/Users/me/notes/next-steps.md", "41 lines"));

    let jobs = vec![
        RunningJob {
            source: None,
            town: Some(town("mark50")),
            job: JobId::try_from("j-k3x9a2").expect("a job id"),
            machine: machine("laptop"),
            owner: address("codex-7f3a", "laptop").into(),
            started_at: ago(now, 3),
            pid: Some(18344),
            command: "cargo test --workspace".into(),
            output: "~/.fastctx/world/towns/mark50/residents/codex-7f3a/out/j-k3x9a2.log".into(),
            stderr: None,
            account: None,
        },
        RunningJob {
            source: None,
            town: Some(town("mark50")),
            job: JobId::try_from("j-p0q1r2").expect("a job id"),
            machine: machine("desktop"),
            owner: address("codex-0d1e", "desktop").into(),
            started_at: ago(now, 382),
            pid: Some(9120),
            command: "python train.py --epochs 400".into(),
            output: "~/.fastctx/world/towns/mark50/residents/codex-0d1e/out/j-p0q1r2.log".into(),
            stderr: None,
            account: None,
        },
    ];
    (vec![claude, codex, finder, outside], jobs)
}

fn towns(now: Timestamp) -> Vec<TownSummary> {
    vec![
        TownSummary {
            town: town("mark50"),
            record: record(
                now,
                "Mark 50 研发",
                "为 Mark 50 战衣研发纳米单元的控制固件，并在真机上验证。",
                &["desktop", "laptop", "server"],
                3,
            ),
            journals: 12,
        },
        TownSummary {
            town: town("fastctx"),
            record: record(
                now,
                "fastctx 重做",
                "世界的核心：机器之间的网络、居民、小镇、共享的账本，以及在任何一台设备上和智能体说话的界面。",
                &["desktop", "laptop"],
                7,
            ),
            journals: 5,
        },
    ]
}

/// Who answered: the desktop with three residents, the laptop with one,
/// and the Hangzhou server offline since a while ago.
fn who(now: Timestamp, scope: WhoScope) -> WorldWho {
    let (residents, jobs) = residents(now);
    let (desktop, laptop): (Vec<_>, Vec<_>) =
        residents.into_iter().partition(|resident| resident.about.machine.as_str() == "desktop");
    let (desktop_jobs, laptop_jobs): (Vec<_>, Vec<_>) =
        jobs.into_iter().partition(|job| job.machine.as_str() == "desktop");
    WorldWho {
        others: Vec::new(),
        involved: Vec::new(),
        history: None,
        you: None,
        machine: machine("desktop"),
        scope,
        machines: vec![
            MachineWho::Answered {
                observation: None,
                reply: WhoReply {
                    history: None,
                    machine: machine("desktop"),
                    residents: desktop,
                    jobs: desktop_jobs,
                    busy: None,
                },
            },
            MachineWho::Answered {
                observation: None,
                reply: WhoReply {
                    history: None,
                    machine: machine("laptop"),
                    residents: laptop,
                    jobs: laptop_jobs,
                    busy: None,
                },
            },
            MachineWho::Offline {
                observation: None,
                machine: machine("server"),
                since: Some(ago(now, 95)),
                last: None,
            },
        ],
    }
}

/// This computer's agents: two Codex logins, the second nearly spent; a
/// Claude Code login read a while ago, and one nobody is logged in to.
fn agents(now: Timestamp) -> fastctx_types::agents::AgentsReply {
    use fastctx_types::agents::*;
    let window = |name: &str, minutes: u32, used_tenths: u32, hours: i64| LimitWindow {
        name: name.into(),
        minutes: Some(minutes),
        used_tenths,
        resets_at: Some(now + SignedDuration::from_hours(hours)),
    };
    let base = |agent: &str, dir: &str, found: HomeFound| AgentFacts {
        agent: agent.parse().unwrap(),
        home: Some(AgentHome { dir: dir.into(), found }),
        account: Fact::unavailable("not logged in"),
        limits: Fact::unavailable("not read"),
        program: Fact::known(Some(AgentProgram {
            path: "C:/Users/me/bin/agent.exe".into(),
            source: ProgramSource::Path,
            exists: true,
        })),
        version: Fact::known("1.0.0".into()),
        launch: Fact::unavailable("not checked"),
        login: Fact::known(LoginStatus::LoggedIn { method: None }),
        models: Fact::unavailable("not listed"),
        integration: Fact::known(Integration::In),
        unattended: Fact::known(true),
        timings: Vec::new(),
        drift: None,
    };
    let account = |email: &str, plan: &str| {
        Fact::known(AgentAccount { email: Some(email.into()), organization: None, plan: Some(plan.into()) })
    };
    let mut codex = base("codex", "C:/Users/me/.codex", HomeFound::Default);
    codex.account = account("me@example.com", "plus");
    codex.limits = Fact::known(AgentLimits {
        observed_at: now - SignedDuration::from_mins(2),
        windows: vec![window("codex", 300, 423, 2), window("codex", 10_080, 182, 80)],
        credits: None,
    });
    let mut work = base("codex", "C:/Users/me/.codex-work", HomeFound::Found);
    work.account = account("me@acme.example", "team");
    work.integration = Fact::known(Integration::Out);
    work.limits = Fact::known(AgentLimits {
        observed_at: now - SignedDuration::from_mins(1),
        windows: vec![window("codex", 300, 1000, 1), window("codex", 10_080, 655, 50)],
        credits: Some(AgentCredits { balance: Some("52230.1460550000".into()), unlimited: false }),
    });
    let mut claude = base("claude", "C:/Users/me/.claude", HomeFound::Default);
    claude.account = account("me@example.com", "max");
    claude.limits = Fact::known(AgentLimits {
        observed_at: now - SignedDuration::from_mins(26),
        windows: vec![window("five_hour", 300, 618, 3), window("seven_day", 10_080, 341, 100)],
        credits: None,
    });
    let mut personal = base("claude", "D:/agents/claude-personal", HomeFound::Added);
    personal.login = Fact::known(LoginStatus::LoggedOut);
    personal.integration = Fact::known(Integration::Out);
    personal.unattended = Fact::known(false);
    AgentsReply {
        machine: "desktop".parse().unwrap(),
        checked_at: now,
        elapsed_ms: 900,
        agents: vec![codex, claude, work, personal],
    }
}

fn nodes(now: Timestamp) -> NodesReply {
    let facts = |name: &str, os: &str, home: &str| MachineFacts {
        turn_debts: false,
        goal_runtime: false,
        goal_colleagues: false,
        goal_waits: false,
        goal_questions: false,
        goal_question_words: false,
        goal_commands: false,
        goal_reports: false,
        sentinels: false,
        sentinel_look: false,
        name: machine(name),
        os: os.into(),
        arch: "x86_64".into(),
        home: home.into(),
        temp: String::new(),
        path_separator: ';',
        case_sensitive: false,
        version: "1.0.0".into(),
        mcp: false,
        mcp_import: false,
        page_accounts: false,
        attachment_places: false,
        attachment_runs: false,
        memory_folders: false,
        collaboration_tree: false,
        completion_receipts: false,
        control_letters: false,
        run_stops: false,
        upload_resumes: false,
        attachment_references: false,
        shell: String::new(),
        form: None,
        node_lifetime: None,
        stats: false,
    };
    let node = |name: &str, online: bool, since: i64, facts: Result<MachineFacts, String>| Node {
        name: machine(name),
        written: name.to_string(),
        online: Some(online),
        since: Some(ago(now, since)),
        through: None,
        allows_direct: false,
        path: None,
        facts,
        owner: Some("me".parse().expect("an account")),
        rights: vec![("me".parse().expect("an account"), fastctx_types::access::Rights { hands: true, agents: true })],
        group: None,
    };
    NodesReply {
        machine: machine("desktop"),
        facts: Some(facts("desktop", "windows", "C:/Users/me")),
        hubs: vec![fastctx_types::world::HubNodes {
            hub: "w-0123456789abcdef".parse().expect("a hub id"),
            name: "home".parse().expect("a hub name"),
            link: LinkState::Online {
                since: ago(now, 300),
                hub: "203.0.113.10:7443".into(),
                through: None,
                ms: None,
                route: Route::Card { name: "Ethernet".into() },
            },
            logins: vec!["me".parse().expect("an account")],
            home: Some("me".parse().expect("an account")),
            here: vec![(
                "me".parse().expect("an account"),
                fastctx_types::access::Rights { hands: true, agents: true },
            )],
            machines: vec![
                node("desktop", true, 300, Ok(facts("desktop", "windows", "C:/Users/me"))),
                node("laptop", true, 180, Ok(facts("laptop", "windows", "C:/Users/me"))),
                node("server", false, 95, Err("it is offline".into())),
            ],
        }],
    }
}

fn map(now: Timestamp) -> MapView {
    let stamp = |minutes: i64, by: Writer, on: &str, id: &str| EntryStamp {
        id: EntryId::try_from(id).expect("an entry id"),
        at: ago(now, minutes),
        machine: machine(on),
        by,
        agent: None,
        model: None,
        session: None,
        cites: Vec::new(),
        attachments: Vec::new(),
    };
    let claude = Writer::Resident { address: address("claude-2b1c", "desktop") };
    let codex = Writer::Resident { address: address("codex-7f3a", "laptop") };
    MapView {
        memory: None,
        pages: Vec::new(),
        town: town("mark50"),
        ledger: "~/.fastctx/world/towns/mark50/ledger".into(),
        record: towns(now).remove(0).record,
        opened: Stamp { by: Writer::User, on: machine("desktop"), at: ago(now, 60 * 24 * 9) },
        journals: vec![
            JournalSummary {
                journal: JournalName::try_from("pid-fixed-point").expect("a journal name"),
                description: Some(Description { version: 2, text: "Porting the PID control loop to fixed-point arithmetic: precision, overflow, timing on the real board.".into(), by: claude.clone(), on: machine("desktop"), at: ago(now, 300) }),
                entries: 7,
                writers: 2,
                latest: Some(stamp(30, codex.clone(), "laptop", "20261003T201205Z-laptop")),
            },
            JournalSummary {
                journal: JournalName::try_from("webrtc-weak-network").expect("a journal name"),
                description: Some(Description { version: 1, text: "WebRTC push streaming over weak and lossy links: loss, freezes, and settings tried.".into(), by: Writer::User, on: machine("desktop"), at: ago(now, 60 * 24 * 4) }),
                entries: 3,
                writers: 3,
                latest: Some(stamp(60 * 26, Writer::User, "desktop", "20261002T183000Z-desktop")),
            },
        ],
        canon: Some(Ok(vec![
            CanonHead { file: "control-loop.md".into(), title: "控制环".into(), scope: "PID 环的结构、采样周期与定点数格式。".into() },
            CanonHead { file: "boards.md".into(), title: "真机".into(), scope: "三块验证板各自的接线、供电与刷写方式。".into() },
        ])),
    }
}

fn journal(now: Timestamp) -> JournalView {
    let entry = |minutes: i64, by: Writer, on: &str, id: &str, model: Option<(&str, bool)>, text: &str| EntryView {
        stamp: EntryStamp {
            id: EntryId::try_from(id).expect("an entry id"),
            at: ago(now, minutes),
            machine: machine(on),
            by,
            agent: None,
            model: model.map(|(name, self_reported)| Model { name: name.into(), self_reported }),
            session: None,
            cites: Vec::new(),
            attachments: Vec::new(),
        },
        text: text.into(),
        mentioned_by: Vec::new(),
    };
    let claude = Writer::Resident { address: address("claude-2b1c", "desktop") };
    let codex = Writer::Resident { address: address("codex-7f3a", "laptop") };
    let mut first = entry(
        180,
        claude,
        "desktop",
        "20261003T171205Z-desktop",
        Some(("claude-opus-5-5", false)),
        "To see whether Q16.16 holds the integral term, ran the loop for 10 min on board B with the step profile: the integral saturated at 41 s and the output clamped for 6 s after. Board B, firmware 0.9.3, gain set 2.",
    );
    first.stamp.attachments = vec![
        Attachment {
            name: "step-profile.log".into(),
            kind: AttachmentKind::Text,
            bytes: 43_008,
            sha256: String::new(),
            kept: Kept::InEntry,
        },
        Attachment {
            name: "scope.png".into(),
            kind: AttachmentKind::Image,
            bytes: 3_250_000,
            sha256: String::new(),
            kept: Kept::OnMachine {
                location: Location { machine: machine("desktop"), path: "D:/scope/run-17.png".into() },
            },
        },
    ];
    first.mentioned_by = vec![EntryId::try_from("20261003T201205Z-laptop").expect("an entry id")];
    let mut second = entry(
        30,
        codex,
        "laptop",
        "20261003T201205Z-laptop",
        Some(("gpt-6.1-sol", true)),
        "Same board and profile with Q8.24 and the integral clamped at ±2^7: no saturation over 10 min; the output settled within 0.4 s of each step. 用户原话：「先别动采样周期」。",
    );
    second.stamp.cites = vec![EntryId::try_from("20261003T171205Z-desktop").expect("an entry id")];
    JournalView {
        town: town("mark50"),
        journal: JournalName::try_from("pid-fixed-point").expect("a journal name"),
        directory: "~/.fastctx/world/towns/mark50/ledger/journals/pid-fixed-point".into(),
        description: map(now).journals[0].description.clone(),
        first: 6,
        entries: vec![first, second],
        total: 7,
        writers: 2,
    }
}

/// A tool use of the World's, on `machine` when it acted on another one.
fn world_tool(name: &str, does: Does, status: ToolStatus, on: Option<&str>) -> What {
    What::Tool(Box::new(Tool {
        run: None,
        name: name.into(),
        server: Some("fastctx".into()),
        machine: on.map(machine),
        does,
        input: serde_json::Value::Null,
        output: Vec::new(),
        status,
        duration_ms: None,
    }))
}

/// The same tool use, having answered `text`.
fn answered_with(what: What, text: &str) -> What {
    match what {
        What::Tool(mut tool) => {
            tool.output = vec![fastctx_types::session::Content::Text { text: text.into() }];
            What::Tool(tool)
        }
        other => other,
    }
}

fn command(text: &str, exit: Option<i64>, job: Option<&str>) -> Does {
    Does::Command { command: text.into(), cwd: None, exit, job: job.map(str::to_string) }
}

/// A conversation with a colleague in the middle of a turn.
fn conversation(now: Timestamp) -> Vec<SessionEvent> {
    let event =
        |seconds: i64, happened: Happened| SessionEvent { at: now - SignedDuration::from_secs(seconds), happened };
    let turn = "t-1".to_string();
    let item = |id: &str, what: What| Item { draft: None, id: id.into(), started_at: None, detail: None, what };
    let used = |seconds: i64, id: &str, what: What| {
        let running = matches!(&what, What::Tool(tool) if tool.status == ToolStatus::Running);
        let mut events = vec![event(seconds, Happened::Began { turn: turn.clone(), item: item(id, what.clone()) })];
        if !running {
            events.push(event(seconds - 2, Happened::Finished { turn: turn.clone(), item: item(id, what) }));
        }
        events
    };
    let steps = |doing: usize| {
        ["Read the loop and the board's timing", "Move the integral to Q8.24", "Run the step tests, then the workspace"]
            .iter()
            .enumerate()
            .map(|(index, text)| Step {
                id: None,
                active_form: None,
                detail: None,
                text: (*text).into(),
                status: match index.cmp(&doing) {
                    std::cmp::Ordering::Less => StepStatus::Done,
                    std::cmp::Ordering::Equal => StepStatus::Doing,
                    std::cmp::Ordering::Greater => StepStatus::Pending,
                },
            })
            .collect::<Vec<_>>()
    };
    let mut events = vec![
        event(900, Happened::TurnStarted { turn: "t-0".into() }),
        event(900, Happened::Heard { message: None,
            delivery: None,
            turn: Some("t-0".into()),
            from: Some(Speaker::Resident { address: address("claude-2b1c", "desktop") }),
            text: "Port the PID loop in D:/work/mark50/src/ctrl/pid.rs to fixed-point. The board has no FPU; keep the sampling period as it is.".into(),
            notice: None,
        }),
        event(860, Happened::Finished { turn: "t-0".into(), item: Item { draft: None, id: "m0".into(), started_at: None, detail: None, what: What::Message { text: "I'll read the loop and the board's timing first.".into() } } }),
        event(600, Happened::TurnEnded { turn: "t-0".into(), end: TurnEnd { facts: None, error: None, how: TurnHow::Completed, reason: None, duration_ms: Some(300_000), last_message: None } }),
        event(240, Happened::TurnStarted { turn: turn.clone() }),
        event(240, Happened::Heard { message: None, delivery: None, turn: Some(turn.clone()), from: Some(Speaker::User { machine: machine("desktop") }), text: "先别动采样周期，积分项用 Q8.24 试试看，跑一下 step 那组测试。".into(), notice: None }),
        event(238, Happened::Finished { turn: turn.clone(), item: item("r1", What::Thinking { text: "**Checking where the clamp sits**\n\nThe accumulator is clamped after it is summed, so an overflow would be cut back only after it happened.".into(), duration_ms: Some(6_400) }) }),
        event(236, Happened::Steps { turn: turn.clone(), steps: steps(1), note: None }),
    ];
    events.extend(used(
        230,
        "c1",
        world_tool(
            "run",
            command("fastctx view -- D:/work/mark50/src/ctrl/pid.rs", Some(0), None),
            ToolStatus::Done,
            None,
        ),
    ));
    let changes = vec![
        Change { path: "D:/work/mark50/src/ctrl/pid.rs".into(), how: ChangeHow::Updated, diff: "@@\n-    integral += error * ki;\n+    let next = integral.saturating_add(error * ki);\n+    integral = next.clamp(-LIMIT, LIMIT);\n".into() },
        Change { path: "D:/work/mark50/src/ctrl/fixed.rs".into(), how: ChangeHow::Added, diff: "+pub struct Q8_24(i32);\n+impl Q8_24 {}\n".into() },
    ];
    events.extend(used(200, "c2", world_tool("apply_patch", Does::Edit { changes }, ToolStatus::Done, None)));
    events.extend(used(
        190,
        "c3",
        world_tool("run", command("cargo build -p ctrl", Some(0), None), ToolStatus::Done, Some("laptop")),
    ));
    events.extend(used(
        150,
        "c4",
        answered_with(
            world_tool(
                "run",
                command("cargo test -p ctrl step_profile", Some(101), None),
                ToolStatus::Done,
                Some("laptop"),
            ),
            "=== run on laptop (exited 101) ===\n   Compiling ctrl v0.3.0 (D:/work/mark50/ctrl)\n    Finished `test` profile in 4.21s\n     Running unittests src/lib.rs\n\nrunning 3 tests\ntest ramp_hold ... ok\ntest noise_reject ... ok\ntest step_profile ... FAILED\n\nfailures:\n---- step_profile stdout ----\nthread 'step_profile' panicked at src/ctrl/pid.rs:88:9:\nintegral saturated at t=41.0s: 2147483647 > 2147483520\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored",
        ),
    ));
    events.push(event(140, Happened::Finished {
        turn: turn.clone(),
        item: item("m1", What::Message { text: "The step test fails at 41 s: the integral still saturates with Q8.24 because the clamp sits after the accumulator. Moving the clamp before it and running the whole workspace.".into() }),
    }));
    events.push(event(138, Happened::Steps { turn: turn.clone(), steps: steps(2), note: None }));
    events.push(event(137, Happened::Context { used: 48_200, window: Some(272_000) }));
    events.push(event(
        130,
        Happened::Health {
            turn: Some(turn.clone()),
            health: Health { kind: HealthKind::Reconnecting, text: "Reconnecting... 2/5".into() },
        },
    ));
    events.extend(used(
        125,
        "c5",
        world_tool(
            "run",
            command("python train.py --epochs 400", None, Some("j-p0q1r2")),
            ToolStatus::Done,
            Some("laptop"),
        ),
    ));
    events.extend(used(
        120,
        "c6",
        world_tool("run", command("cargo test --workspace", None, None), ToolStatus::Running, Some("laptop")),
    ));
    events.push(event(
        8,
        Happened::Began {
            turn: turn.clone(),
            item: item("r2", What::Thinking { text: String::new(), duration_ms: None }),
        },
    ));
    events.push(event(
        6,
        Happened::Grew { revision: None, turn: turn.clone(), item: "r2".into(), part: Part::Text, text: "**Waiting on the workspace tests**\n\nWhile they run, the noise_reject margin is worth a look: 0.3% is inside".into() },
    ));
    events
}

const ANSWER: &str = "## 定点化完成

积分项改成 **Q8.24** 之后，`step_profile` 在 41 s 处的饱和消失了。原因是限幅放在累加之后：

1. 累加器先溢出，再被限幅截回；
2. 截回的值已经错了，后面每一拍都带着它。

把限幅挪到累加之前：

```rust
let next = self.integral.saturating_add(error * self.ki);
self.integral = next.clamp(-LIMIT, LIMIT);
```

### 验证结果

| 测试 | 浮点 | Q8.24 |
|---|---|---|
| step_profile | 通过 | 通过 |
| ramp_hold | 通过 | 通过 |
| noise_reject | 通过 | 误差 0.3% |

> 采样周期没动，仍是 1 ms。

还剩 `noise_reject` 那 0.3%，要不要再看，你定。";
const TABLE_ANSWER: &str = "## 验证明细

| 检查项 | 工作节点 | 运行阶段 | 结果 |
|---|---|---|---|
| accumulator_boundary_guard checks the upper and lower limits under sustained input | controller_primary on revision B | fixed_point_validation with steady input | completed_without_error after saturation |
| accumulator_boundary_guard checks recovery after saturation | controller_secondary on revision C | fixed_point_validation with noisy input | completed_without_error after recovery |

> 两块板子的采样周期都是 1 ms。";

/// A finished turn whose answer is written the way agents write: a
/// heading, lists, code, a table.
fn answered(now: Timestamp, answer: &str) -> Vec<SessionEvent> {
    let event =
        |seconds: i64, happened: Happened| SessionEvent { at: now - SignedDuration::from_secs(seconds), happened };
    let turn = "t-2".to_string();
    let tested = Item {
        draft: None,
        started_at: None,
        detail: None,
        id: "c1".into(),
        what: world_tool("run", command("cargo test --workspace", Some(0), None), ToolStatus::Done, None),
    };
    vec![
        event(400, Happened::TurnStarted { turn: turn.clone() }),
        event(
            400,
            Happened::Heard {
                message: None,
                delivery: None,
                turn: Some(turn.clone()),
                from: Some(Speaker::User { machine: machine("desktop") }),
                text: "限幅的问题修好了吗？把结果列一下。".into(),
                notice: None,
            },
        ),
        event(390, Happened::Began { turn: turn.clone(), item: tested.clone() }),
        event(330, Happened::Finished { turn: turn.clone(), item: tested }),
        event(
            320,
            Happened::Finished {
                turn: turn.clone(),
                item: Item {
                    draft: None,
                    id: "m3".into(),
                    started_at: None,
                    detail: None,
                    what: What::Message { text: answer.into() },
                },
            },
        ),
        event(
            320,
            Happened::TurnEnded {
                turn,
                end: TurnEnd {
                    facts: None,
                    error: None,
                    how: TurnHow::Completed,
                    reason: None,
                    duration_ms: Some(80_000),
                    last_message: None,
                },
            },
        ),
    ]
}

/// Every scene, by name.
pub fn scenes(ctx: &Ctx) -> Vec<(&'static str, Page)> {
    let now = ctx.now;
    let world_view = TownView::new(who(now, WhoScope::World { towns: towns(now) }), None);
    let mut in_town =
        who(now, WhoScope::Town { town: town("mark50"), record: Some(Box::new(towns(now).remove(0).record)) });
    for machine in &mut in_town.machines {
        if let MachineWho::Answered { reply, .. } = machine {
            reply.residents.retain(|resident| resident.town.is_some());
        }
    }
    let town_view = TownView::new(in_town, None);

    let colleague = town_view
        .residents
        .iter()
        .find(|resident| resident.about.id.as_str() == "codex-7f3a")
        .cloned()
        .expect("the colleague");
    let mut talk = TalkPage::new(colleague.clone(), Some("Mark 50 研发".into()), Some(machine("desktop")));
    talk.data(ctx, Data::History(conversation(now)));
    let mut detailed = TalkPage::new(colleague, Some("Mark 50 研发".into()), Some(machine("desktop")));
    detailed.data(ctx, Data::History(conversation(now)));
    detailed.key(ctx, KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    let mut answer =
        TalkPage::new(town_view.residents[0].clone(), Some("Mark 50 研发".into()), Some(machine("desktop")));
    answer.data(ctx, Data::History(answered(now, ANSWER)));
    let mut table_answer =
        TalkPage::new(town_view.residents[0].clone(), Some("Mark 50 研发".into()), Some(machine("desktop")));
    table_answer.data(ctx, Data::History(answered(now, TABLE_ANSWER)));
    // The Claude that asked for both Codex colleagues, with them below its
    // conversation, then with the first one picked.
    let asker = address("claude-2b1c", "desktop");
    let around = crate::app::Around {
        jobs: town_view.jobs_by(&asker).into_iter().cloned().collect(),
        colleagues: town_view.colleagues_of(&asker).into_iter().cloned().collect(),
    };
    let mut asking =
        TalkPage::new(town_view.residents[0].clone(), Some("Mark 50 研发".into()), Some(machine("desktop")));
    asking.data(ctx, Data::History(answered(now, ANSWER)));
    asking.data(ctx, Data::Around(Ok(around)));
    let mut picking =
        TalkPage::new(town_view.residents[0].clone(), Some("Mark 50 研发".into()), Some(machine("desktop")));
    picking.data(ctx, Data::History(answered(now, ANSWER)));
    picking.data(
        ctx,
        Data::Around(Ok(crate::app::Around {
            jobs: Vec::new(),
            colleagues: town_view.colleagues_of(&asker).into_iter().cloned().collect(),
        })),
    );
    picking.key(ctx, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    picking.key(ctx, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let mut typing =
        TalkPage::new(town_view.residents[0].clone(), Some("Mark 50 研发".into()), Some(machine("desktop")));
    typing.data(ctx, Data::History(Vec::new()));
    typing.paste("跑完以后把结果写进 pid-fixed-point 那份手记，\n条件写清楚：板子、固件版本、增益。".into());

    let mut open = FormPage::open_agent(Some(town("mark50")));
    open.data(ctx, Data::Choices { nodes: Ok(nodes(now)), towns: Ok(towns(now)), assisting: Vec::new() });
    for _ in 0..3 {
        open.key(ctx, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }
    open.paste("看一下 laptop 上 /data/mark50-sim 里最新那批仿真数据，有没有哪一组的超调明显偏大。".into());

    let mut new_project = FormPage::new_project();
    new_project.data(ctx, Data::Choices { nodes: Ok(nodes(now)), towns: Ok(towns(now)), assisting: Vec::new() });
    new_project.paste("probe".into());
    new_project.key(ctx, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    new_project.paste("深空探测器".into());

    let mut machines = MachinesPage::with(nodes(now));
    machines.data(Data::Sessions(Ok(crate::sessions::Status {
        codex: crate::sessions::Standing::In,
        claude: crate::sessions::Standing::Out,
        codex_tools: Some(Box::new(fastctx_install::apply::ToolCheck {
            host: "Codex",
            scope: "effective Codex features in this directory".into(),
            result: Ok(()),
        })),
        claude_tools: None,
        legacy: Some(fastctx_install::upgrade::Summary {
            record: "C:/Users/me/.fastctx/upgrade-clean.json".into(),
            archives: vec!["C:/Users/me/.fastctx/legacy-archive-2026-10-04".into()],
            archived: 14,
            tidied: 6,
            waiting: 1,
        }),
    })));

    machines.data(Data::Agents(Ok(agents(now))));

    // This computer just after the network it was on was dissolved.
    let mut alone = nodes(now);
    alone.hubs.clear();
    let mut dissolved = MachinesPage::with(alone);
    dissolved.data(Data::Agents(Ok(agents(now))));
    let lab = fastctx_types::accounts::Dissolution {
        hub: "lab".parse().expect("a name"),
        at: now.checked_sub(jiff::SignedDuration::from_mins(3)).expect("in range"),
    };
    dissolved.data(Data::Accounts(Vec::new(), vec![lab]));

    vec![
        ("world", Page::World(Box::new(WorldPage::with(world_view, nodes(now))))),
        ("project", Page::Project(Box::new(ProjectPage::with(town("mark50"), town_view, map(now))))),
        ("talk", Page::Talk(Box::new(talk))),
        ("talk-details", Page::Talk(Box::new(detailed))),
        ("answer", Page::Talk(Box::new(answer))),
        ("answer-table", Page::Talk(Box::new(table_answer))),
        ("colleagues", Page::Talk(Box::new(asking))),
        ("colleague-picked", Page::Talk(Box::new(picking))),
        ("typing", Page::Talk(Box::new(typing))),
        ("journal", Page::Journal(Box::new(JournalPage::with("Mark 50 研发".into(), journal(now))))),
        ("machines", Page::Machines(Box::new(machines))),
        ("machines-dissolved", Page::Machines(Box::new(dissolved))),
        ("open-agent", Page::Form(Box::new(open))),
        ("new-project", Page::Form(Box::new(new_project))),
    ]
}
