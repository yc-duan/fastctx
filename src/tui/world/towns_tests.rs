//! Rendering health of the Towns page and the chat screen, and their render-review scenes.

use super::talk::{TalkClient, Transcript};
use super::tests::{draw, export_scene, fixture};
use super::towns::{Census, Chat, Compose, Field, TownName};
use super::{Page, Screen, WorldApp};
use crate::control::i18n::{ALL_LANGUAGES, Language};
use crate::town::records::{About, Birth, MessageLine, Now, Trouble};
use crate::town::talk::TalkEvent;
use crate::town::w::{Silence, Silent};
use crate::town::who::{ActionLine, JobFacts, ResidentFacts};

fn ago(seconds: i64) -> String {
    (time::OffsetDateTime::now_utc() - time::Duration::seconds(seconds))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

fn resident(id: &str, agent: &str, model: Option<&str>, town: Option<&str>, state: &str, birth: Birth, surface: &str) -> ResidentFacts {
    ResidentFacts {
        about: About {
            id: id.into(),
            agent: agent.into(),
            agent_version: None,
            model: model.map(str::to_string),
            model_source: model.map(|_| "host".to_string()),
            machine: "desktop".into(),
            surface: surface.into(),
            birth,
            since: ago(7800),
            session: None,
            town: town.map(str::to_string),
        },
        now: Now {
            state: state.into(),
            since: ago(120),
            running: None,
            last_action: Some("write D:/mark50/specs/world-town.md".into()),
            last_action_at: Some(ago(65)),
            ..Now::default()
        },
        dir: std::path::PathBuf::from("C:/Users/Ada/.fastctx/world/towns/mark50/residents").join(id),
        actions: Vec::new(),
        actions_total: 0,
        messages: Vec::new(),
        damaged_messages: 0,
    }
}

pub(super) fn census(residents: Vec<ResidentFacts>) -> Census {
    Census {
        machine: "desktop".into(),
        machines: vec!["desktop".into(), "laptop".into()],
        silent: vec![Silent { machine: "hangzhou".into(), why: Silence::Offline(Some(ago(3600))) }],
        alone: false,
        residents,
        running: Vec::new(),
        towns: vec![
            TownName { slug: "mark50".into(), name: "mark50 研发".into(), machines: vec!["desktop".into(), "laptop".into()] },
            TownName { slug: "probe".into(), name: "probe".into(), machines: vec!["desktop".into()] },
        ],
        detail: None,
    }
}

fn populated() -> Census {
    let colleague = Birth::Colleague { requester: "claude-2b1c".into(), label: "port the PID loop to fixed-point".into(), machine: None };
    let mut residents = vec![
        resident("codex-7f3a", "codex", Some("gpt-6-astra"), Some("mark50"), "busy", colleague.clone(), "world"),
        resident("claude-2b1c", "claude-code", Some("opus-5-5"), Some("mark50"), "idle", Birth::User, "vscode"),
        resident("codex-91c0", "codex", Some("gpt-6-astra"), Some("mark50"), "asleep", colleague, "world"),
        resident("codex-0d1e", "codex", Some("gpt-6-astra"), Some("probe"), "interrupted", Birth::User, "world"),
        resident("claude-5e77", "claude-code", None, None, "unknown", Birth::User, "cli"),
    ];
    residents[0].now.desk = true;
    residents[0].now.trouble = Some(Trouble {
        at: ago(240),
        text: "OpenAI is holding this turn's output for extra safety processing (use: cyber; reason: probes).".into(),
    });
    residents[2].about.machine = "laptop".into();
    let mut census = census(residents);
    census.running.push(JobFacts {
        resident: "codex-7f3a".into(),
        town: Some("mark50".into()),
        id: "j-k3x9a2".into(),
        pid: Some(18344),
        command: "cargo test --workspace".into(),
        machine: "desktop".into(),
        started_at: ago(190),
        ended_at: None,
        state: "running".into(),
        exit_code: None,
        output_path: "C:/Users/Ada/.fastctx/world/towns/mark50/residents/codex-7f3a/out/j-k3x9a2.log".into(),
    });
    let mut detail = census.residents[0].clone();
    for (index, (hand, what, result)) in [
        ("run", "rg -n encode_frame D:/mark50/src", "exit 0"),
        ("run", "fastctx view -- D:/mark50/src/ctrl/pid.rs D:/mark50/src/ctrl/loop.rs", "exit 0"),
        ("edit", "D:/mark50/src/ctrl/pid.rs", "1 changed"),
        ("run", "cargo build", "exit 0"),
        ("apply_patch", "D:/mark50/src/ctrl/fixed.rs", "1 changed"),
        ("run", "cargo test -p ctrl", "exit 101"),
        ("run", "cargo test --workspace", "running j-k3x9a2"),
    ]
    .into_iter()
    .enumerate()
    {
        detail.actions.push(ActionLine { at: ago(600 - index as i64 * 60), machine: "desktop".into(), hand: hand.into(), what: what.into(), result: result.into() });
    }
    detail.actions_total = 41;
    detail.messages.push(MessageLine {
        at: ago(900),
        kind: "answered".into(),
        peer: "claude-2b1c".into(),
        turn: Some("t1".into()),
        text: Some("The fixed-point PID compiles; two tests still fail on saturation. Details in the journal.".into()),
    });
    census.detail = Some(detail);
    census
}

fn transcript() -> Transcript {
    let mut transcript = Transcript::default();
    for event in [
        TalkEvent::Words { from: "user".into(), text: "把 PID 控制环移植到定点数，先跑通 ctrl 的测试。".into(), item: None },
        TalkEvent::TurnStarted { turn: "t1".into() },
        TalkEvent::Tool { item: "i1".into(), name: "fastctx.run".into(), summary: "rg -n encode_frame D:/mark50/src".into(), done: true, failed: false },
        TalkEvent::Message { item: "i2".into(), text: "Found 14 callers. The loop in src/ctrl/loop.rs is the only one on the hot path.\nI'll start there.".into() },
        TalkEvent::Tool { item: "i3".into(), name: "fastctx.apply_patch".into(), summary: "D:/mark50/src/ctrl/fixed.rs".into(), done: true, failed: false },
        TalkEvent::Tool { item: "i4".into(), name: "fastctx.run".into(), summary: "cargo test -p ctrl".into(), done: true, failed: true },
        TalkEvent::TurnEnded { turn: "t1".into(), status: "completed".into(), reason: None },
        TalkEvent::Words { from: "claude-2b1c".into(), text: "Saturation tests matter most; keep the old float path behind a feature.".into(), item: None },
        TalkEvent::Live,
        TalkEvent::TurnStarted { turn: "t2".into() },
        TalkEvent::Tool { item: "i5".into(), name: "fastctx.run".into(), summary: "cargo test --workspace".into(), done: false, failed: false },
        TalkEvent::Delta { item: "i6".into(), text: "Running the whole workspace now; the saturation".into() },
    ] {
        transcript.push(event);
    }
    transcript
}

fn towns_app() -> WorldApp {
    let mut app = fixture();
    app.page = Page::Towns;
    app.towns.census = Some(Ok(populated()));
    app
}

fn chat_app() -> WorldApp {
    let mut app = towns_app();
    app.towns.chat = Some(Chat::new(Some(TalkClient::detached("codex-7f3a", transcript())), None));
    app.screen = Screen::Talk;
    app
}

fn compose_app() -> WorldApp {
    let mut app = towns_app();
    app.towns.chat = Some(Chat::new(None, Some(Compose { agent: 0, machine: 0, town: 1, focus: Field::Machine })));
    app.screen = Screen::Talk;
    app
}

#[test]
fn every_language_draws_the_towns_and_the_chat_at_every_size() {
    for language in ALL_LANGUAGES {
        for (width, height) in [(39, 8), (40, 9), (52, 12), (80, 24), (120, 36)] {
            for screen in [Screen::Page, Screen::Detail, Screen::Actions, Screen::Help] {
                let mut app = towns_app();
                app.screen = screen;
                draw(&mut app, language, width, height);
            }
            draw(&mut chat_app(), language, width, height);
            draw(&mut compose_app(), language, width, height);
        }
    }
}

/// Exports the Towns and chat scenes for the render review; see `export_world_render_scenes`.
///
/// ```text
/// cargo test --lib tui::world::towns_tests::export_town_render_scenes -- --ignored --exact
/// ```
#[test]
#[ignore = "developer tool: exports render-review scenes, asserts nothing"]
fn export_town_render_scenes() {
    for (name, language, width, height) in [("towns-zh", Language::ZhCn, 120, 36), ("towns-en-100", Language::En, 100, 30), ("towns-80", Language::ZhCn, 80, 24), ("towns-40", Language::ZhCn, 40, 9)] {
        export_scene(name, language, width, height, towns_app());
    }
    let mut empty = fixture();
    empty.page = Page::Towns;
    empty.towns.census = Some(Ok(census(Vec::new())));
    export_scene("towns-empty-zh", Language::ZhCn, 100, 30, empty);
    let mut detail = towns_app();
    detail.screen = Screen::Detail;
    export_scene("towns-detail-zh", Language::ZhCn, 100, 30, detail);
    for (name, language, width, height) in [("chat-zh", Language::ZhCn, 100, 30), ("chat-en-80", Language::En, 80, 24), ("chat-40", Language::ZhCn, 40, 9)] {
        export_scene(name, language, width, height, chat_app());
    }
    export_scene("chat-compose-zh", Language::ZhCn, 100, 30, compose_app());
    let mut elsewhere = compose_app();
    if let Some(compose) = elsewhere.towns.chat.as_mut().and_then(|chat| chat.compose.as_mut()) {
        compose.machine = 1;
        compose.town = 1;
    }
    export_scene("chat-compose-laptop-en-80", Language::En, 80, 24, elsewhere);
    let mut ended = towns_app();
    let mut transcript = transcript();
    for event in [
        TalkEvent::Health { text: "OpenAI is holding this turn's output for extra safety processing (use: cyber; reason: probes).".into() },
        TalkEvent::TurnEnded { turn: "t2".into(), status: "failed".into(), reason: Some("This content was flagged for possible cybersecurity risk.".into()) },
    ] {
        transcript.push(event);
    }
    ended.towns.chat = Some(Chat::new(Some(TalkClient::detached("codex-7f3a", transcript)), None));
    ended.screen = Screen::Talk;
    export_scene("chat-ended-zh", Language::ZhCn, 100, 30, ended);
    for (name, language, width, height) in [("project-form-zh", Language::ZhCn, 100, 30), ("project-form-en-80", Language::En, 80, 24)] {
        let mut app = towns_app();
        let mut form = super::forms::Form::new(super::forms::FormKind::Project, "desktop, laptop");
        for (index, value) in ["mark50", "mark50 研发", "为 Mark 50 战衣研发纳米单元的控制固件，并在真机上验证。"].iter().enumerate() {
            form.fields[index].input.replace((*value).to_string());
        }
        form.fields[4].input.replace("固件源码 | desktop | D:/work/mark50
仿真数据 | laptop | /data/mark50-sim".to_string());
        app.form = Some(form);
        app.screen = Screen::Form;
        export_scene(name, language, width, height, app);
    }
}
