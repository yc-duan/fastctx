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
use fastctx_types::session::{Happened, Health, HealthKind, SessionEvent, Speaker, TurnEnd, TurnHow};
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
        about: About {
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
            model: Some(model.to_string()),
        },
        town: in_town.map(town),
        here: true,
        now: None,
        last_action: None,
        moved: None,
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
    }
}

fn action(now: Timestamp, minutes: i64, on: &str, hand: &str, what: &str, result: &str) -> Action {
    Action {
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
                location: Location { machine: machine("hangzhou"), path: "/srv/mark50-ci".into() },
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
            job: JobId::try_from("j-k3x9a2").expect("a job id"),
            machine: machine("laptop"),
            owner: address("codex-7f3a", "laptop"),
            started_at: ago(now, 3),
            pid: Some(18344),
            command: "cargo test --workspace".into(),
            output: "~/.fastctx/world/towns/mark50/residents/codex-7f3a/out/j-k3x9a2.log".into(),
        },
        RunningJob {
            job: JobId::try_from("j-p0q1r2").expect("a job id"),
            machine: machine("desktop"),
            owner: address("codex-0d1e", "desktop"),
            started_at: ago(now, 382),
            pid: Some(9120),
            command: "python train.py --epochs 400".into(),
            output: "~/.fastctx/world/towns/mark50/residents/codex-0d1e/out/j-p0q1r2.log".into(),
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
                &["desktop", "laptop", "hangzhou"],
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
        machine: machine("desktop"),
        scope,
        machines: vec![
            MachineWho::Answered {
                reply: WhoReply { machine: machine("desktop"), residents: desktop, jobs: desktop_jobs },
            },
            MachineWho::Answered {
                reply: WhoReply { machine: machine("laptop"), residents: laptop, jobs: laptop_jobs },
            },
            MachineWho::Offline { machine: machine("hangzhou"), since: Some(ago(now, 95)) },
        ],
    }
}

fn nodes(now: Timestamp) -> NodesReply {
    let facts = |name: &str, os: &str, home: &str| MachineFacts {
        name: machine(name),
        os: os.into(),
        arch: "x86_64".into(),
        home: home.into(),
        temp: String::new(),
        path_separator: ';',
        case_sensitive: false,
        version: "1.0.0".into(),
    };
    NodesReply {
        machine: machine("desktop"),
        world: Some("w-4f1c".into()),
        link: LinkState::Online {
            since: ago(now, 300),
            hub: "121.40.82.28:7443".into(),
            route: Route::Card { name: "以太网 2".into() },
        },
        machines: vec![
            Node {
                name: machine("desktop"),
                online: Some(true),
                since: Some(ago(now, 300)),
                facts: Ok(facts("desktop", "windows", "C:/Users/me")),
                revoked: None,
            },
            Node {
                name: machine("laptop"),
                online: Some(true),
                since: Some(ago(now, 180)),
                facts: Ok(facts("laptop", "windows", "C:/Users/me")),
                revoked: None,
            },
            Node {
                name: machine("hangzhou"),
                online: Some(false),
                since: Some(ago(now, 95)),
                facts: Err("it is offline".into()),
                revoked: None,
            },
        ],
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

/// A conversation with a colleague in the middle of a turn.
fn conversation(now: Timestamp) -> Vec<SessionEvent> {
    let event =
        |seconds: i64, happened: Happened| SessionEvent { at: now - SignedDuration::from_secs(seconds), happened };
    let turn = "t-1".to_string();
    let tool = |seconds: i64, item: &str, tool: &str, detail: &str, ended: Option<bool>| {
        let mut events = vec![event(
            seconds,
            Happened::ToolStarted { turn: turn.clone(), item: item.into(), tool: tool.into(), detail: detail.into() },
        )];
        if let Some(failed) = ended {
            events.push(event(
                seconds - 2,
                Happened::ToolEnded {
                    turn: turn.clone(),
                    item: item.into(),
                    tool: tool.into(),
                    detail: detail.into(),
                    failed,
                },
            ));
        }
        events
    };
    let mut events = vec![
        event(900, Happened::TurnStarted { turn: "t-0".into() }),
        event(900, Happened::Heard {
            turn: Some("t-0".into()),
            from: Some(Speaker::Resident { address: address("claude-2b1c", "desktop") }),
            text: "Port the PID loop in D:/work/mark50/src/ctrl/pid.rs to fixed-point. The board has no FPU; keep the sampling period as it is.".into(),
        }),
        event(860, Happened::Said { turn: "t-0".into(), item: "m0".into(), text: "I'll read the loop and the board's timing first.".into() }),
        event(600, Happened::TurnEnded { turn: "t-0".into(), end: TurnEnd { how: TurnHow::Completed, reason: None, duration_ms: Some(300_000), last_message: None } }),
        event(240, Happened::TurnStarted { turn: turn.clone() }),
        event(240, Happened::Heard { turn: Some(turn.clone()), from: Some(Speaker::User { machine: machine("desktop") }), text: "先别动采样周期，积分项用 Q8.24 试试看，跑一下 step 那组测试。".into() }),
    ];
    events.extend(tool(230, "c1", "fastctx.view", "D:/work/mark50/src/ctrl/pid.rs", Some(false)));
    events.extend(tool(200, "c2", "apply_patch", "M src/ctrl/pid.rs, A src/ctrl/fixed.rs", Some(false)));
    events.extend(tool(190, "c3", "fastctx.run", "cargo build -p ctrl", Some(false)));
    events.extend(tool(150, "c4", "fastctx.run", "cargo test -p ctrl step_profile", Some(true)));
    events.push(event(140, Happened::Said {
        turn: turn.clone(),
        item: "m1".into(),
        text: "The step test fails at 41 s: the integral still saturates with Q8.24 because the clamp sits after the accumulator. Moving the clamp before it and running the whole workspace.".into(),
    }));
    events.push(event(
        130,
        Happened::Health {
            turn: Some(turn.clone()),
            health: Health { kind: HealthKind::Reconnecting, text: "Reconnecting... 2/5".into() },
        },
    ));
    events.extend(tool(120, "c5", "fastctx.run", "cargo test --workspace", None));
    events.push(event(
        5,
        Happened::Writing {
            turn: turn.clone(),
            item: "m2".into(),
            delta: "While the workspace tests run, the clamp now limits the accumulator to ±2^7, and".into(),
        },
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
    vec![
        event(400, Happened::TurnStarted { turn: turn.clone() }),
        event(
            400,
            Happened::Heard {
                turn: Some(turn.clone()),
                from: Some(Speaker::User { machine: machine("desktop") }),
                text: "限幅的问题修好了吗？把结果列一下。".into(),
            },
        ),
        event(
            390,
            Happened::ToolStarted {
                turn: turn.clone(),
                item: "c1".into(),
                tool: "fastctx.run".into(),
                detail: "cargo test --workspace".into(),
            },
        ),
        event(
            330,
            Happened::ToolEnded {
                turn: turn.clone(),
                item: "c1".into(),
                tool: "fastctx.run".into(),
                detail: "cargo test --workspace".into(),
                failed: false,
            },
        ),
        event(320, Happened::Said { turn: turn.clone(), item: "m3".into(), text: answer.into() }),
        event(
            320,
            Happened::TurnEnded {
                turn,
                end: TurnEnd { how: TurnHow::Completed, reason: None, duration_ms: Some(80_000), last_message: None },
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
        if let MachineWho::Answered { reply } = machine {
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
    let mut talk = TalkPage::new(colleague, Some("Mark 50 研发".into()), Some(machine("desktop")));
    talk.data(ctx, Data::History(conversation(now)));
    let mut answer =
        TalkPage::new(town_view.residents[0].clone(), Some("Mark 50 研发".into()), Some(machine("desktop")));
    answer.data(ctx, Data::History(answered(now, ANSWER)));
    let mut table_answer =
        TalkPage::new(town_view.residents[0].clone(), Some("Mark 50 研发".into()), Some(machine("desktop")));
    table_answer.data(ctx, Data::History(answered(now, TABLE_ANSWER)));
    let mut typing =
        TalkPage::new(town_view.residents[0].clone(), Some("Mark 50 研发".into()), Some(machine("desktop")));
    typing.data(ctx, Data::History(Vec::new()));
    typing.paste("跑完以后把结果写进 pid-fixed-point 那份手记，\n条件写清楚：板子、固件版本、增益。".into());

    let mut open = FormPage::open_agent(Some(town("mark50")));
    open.data(ctx, Data::Choices { nodes: Ok(nodes(now)), towns: Ok(towns(now)) });
    for _ in 0..3 {
        open.key(ctx, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }
    open.paste("看一下 laptop 上 /data/mark50-sim 里最新那批仿真数据，有没有哪一组的超调明显偏大。".into());

    let mut new_project = FormPage::new_project();
    new_project.data(ctx, Data::Choices { nodes: Ok(nodes(now)), towns: Ok(towns(now)) });
    new_project.paste("probe".into());
    new_project.key(ctx, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    new_project.paste("深空探测器".into());

    vec![
        ("world", Page::World(WorldPage::with(world_view, nodes(now)))),
        ("project", Page::Project(Box::new(ProjectPage::with(town("mark50"), town_view, map(now))))),
        ("talk", Page::Talk(Box::new(talk))),
        ("answer", Page::Talk(Box::new(answer))),
        ("answer-table", Page::Talk(Box::new(table_answer))),
        ("typing", Page::Talk(Box::new(typing))),
        ("journal", Page::Journal(Box::new(JournalPage::with("Mark 50 研发".into(), journal(now))))),
        ("machines", Page::Machines(MachinesPage::with(nodes(now)))),
        ("open-agent", Page::Form(Box::new(open))),
        ("new-project", Page::Form(Box::new(new_project))),
    ]
}
