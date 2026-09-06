//! Cross-language, cross-size rendering health for the World control surface.

use super::*;
use crate::control::i18n::{ALL_LANGUAGES, Language};
use crate::world::client::{Disk, Inventory};
use crate::world::control::{FailureKind, NodeSnapshot};
use crate::world::grant::{Grant, GrantEntry, GrantSnapshot};
use crate::world::node::activity::NodeActivity;
use crate::world::node::admin::NodeStatus;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn fixture() -> WorldApp {
    fixture_with(3)
}

fn fixture_with(machines: usize) -> WorldApp {
    let paths = ControlPaths::for_home(std::env::temp_dir().join("fastctx-world-render-only"));
    let mut app = WorldApp::new(&paths);
    let now = crate::world::now_rfc3339();
    let status: NodeStatus = serde_json::from_value(serde_json::json!({
        "name":"desktop", "world_id":"world-demo", "hub":["example.test:443"], "hub_key":"public fingerprint", "fingerprint":"public fingerprint", "pid":1234,
        "version":"1.0.0", "started_at":now, "written_at":now,
        "link":{"state":"connected","network":"direct","path":"direct via Ethernet","interface":"Ethernet", "tunnels":[],"tls":"pinned", "connected_since":now,"last_heartbeat_at":now,"last_ack_at":now,"rtt_ms":18,"hub_time_offset_s":0,"last_error":null,"replaced_count":0,"key_epoch":2,"outbox_depth":0},
        "members":machines,"members_online":machines.saturating_sub(1),"grant_revision":1,"grant_stale":false,"running_calls":1,"engine_hosted":true,"agent_connections":2
    })).unwrap();
    let inventory = Inventory {
        hostname: "workstation".into(),
        os: "windows".into(),
        arch: "x86_64".into(),
        cpus: 16,
        memory_gb: 32.0,
        home: "C:/Users/Ada".into(),
        user: "Ada".into(),
        description: "办公台式机 · builds and experiments".into(),
        collected_at: now.clone(),
        disks: vec![Disk {
            mount: "C:".into(),
            free_gb: 120.5,
            total_gb: 476.0,
        }],
        activity: Some(NodeActivity {
            engine_hosted: true,
            agent_connections: 2,
            local_requests: 1,
            remote_calls: 0,
            observed_at: now.clone(),
        }),
        ..Inventory::default()
    };
    let first = NodeView {
        name: "desktop".into(),
        state: "online".into(),
        last_seen: now.clone(),
        os: "windows".into(),
        arch: "x86_64".into(),
        tags: vec!["office".into()],
        version: "1.0.0".into(),
        hub_rtt_ms: Some(18),
        tls: Some("pinned".into()),
        network: Some("direct".into()),
        inventory: Some(inventory),
        is_self: true,
        private_note: None,
    };
    let mut nodes = vec![first.clone()];
    for index in 1..machines {
        let mut other = first.clone();
        other.name = format!("machine-{index}");
        other.os = if index % 2 == 0 { "linux" } else { "macos" }.into();
        other.is_self = false;
        other.private_note = Some("我的服务器".into());
        if index % 3 == 2 {
            other.state = "offline".into();
            other.hub_rtt_ms = None;
        }
        nodes.push(other);
    }
    let mut config: crate::world::WorldConfig=serde_json::from_value(serde_json::json!({"version":1,"name":"desktop","world_id":"world-demo","hub":["example.test:443"],"hub_key":"public fingerprint","tls":"pinned","enrolled_at":now})).unwrap();
    config.network = NetworkMode::Direct;
    let node = NodeSnapshot {
        status: status.clone(),
        nodes: ReadState::Ready(nodes),
        grants: GrantSnapshot {
            revision: 1,
            grants: vec![GrantEntry {
                id: "g-a12345".into(),
                grant: Grant {
                    principal: "future:web-client".into(),
                    nodes: vec!["tag:office".into()],
                    verbs: vec!["grep".into(), "glob".into()],
                    expires: None,
                },
            }],
            published_by: "desktop".into(),
            published_at: now.clone(),
        },
        description: ReadState::Ready("办公台式机 · builds and experiments".into()),
        unverified: Vec::new(),
    };
    let hub:crate::world::hub::HubStatus=serde_json::from_value(serde_json::json!({"version":"1.0.0","pid":1000,"written_at":now,"started_at":now,"listen":"0.0.0.0:443","world_id":"world-demo","hub_key":"public fingerprint","tls":"pinned","binding":"direct","members":[{"name":"desktop","state":"online","last_seen":now,"outbox":0,"version":"1.0.0"}],"open_invites":2,"events":30,"rotation_pending":false,"bootstrap_used":true,"data_connections":2})).unwrap();
    app.snapshot = Some(Snapshot {
        observed_at: now.clone(),
        config: ReadState::Ready(config),
        node: ReadState::Ready(node),
        last_status: ReadState::Ready(status),
        description: ReadState::Ready("办公台式机 · builds and experiments".into()),
        service_installed: ReadState::Ready(true),
        hub: ReadState::Ready(hub),
        hub_data: app.hub_data.clone(),
    });
    app.events = vec![
        Event {
            seq: 1,
            at: now.clone(),
            subject: "desktop".into(),
            kind: "node.online".into(),
            facts: Default::default(),
        },
        Event {
            seq: 2,
            at: now,
            subject: "machine-1".into(),
            kind: "grant.changed".into(),
            facts: Default::default(),
        },
    ];
    app.event_cursor = 2;
    app.event_latest = 2;
    app.events_loaded = true;
    app.rtt_history = (0..40).map(|index| 12 + (index * 7) % 23).collect();
    app
}

use crate::world::NetworkMode;

fn draw(app: &mut WorldApp, language: Language, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| super::render(frame, app, language, frame.area()))
        .unwrap();
    terminal
}

#[test]
fn all_languages_render_world_states_at_terminal_boundaries() {
    for language in ALL_LANGUAGES {
        for (width, height) in [(39, 8), (40, 9), (52, 12), (80, 24), (120, 36)] {
            for page in Page::ALL {
                for machines in [1, 3, 9] {
                    let mut app = fixture_with(machines);
                    app.page = page;
                    draw(&mut app, language, width, height);
                    app.screen = Screen::Detail;
                    draw(&mut app, language, width, height);
                    app.screen = Screen::Actions;
                    draw(&mut app, language, width, height);
                    app.screen = Screen::Help;
                    draw(&mut app, language, width, height);
                }
            }
            for state in 0..7 {
                let mut app = fixture();
                match state {
                    0 => app.snapshot = None,
                    1 => app.snapshot.as_mut().unwrap().config = ReadState::Missing,
                    2 => {
                        app.snapshot.as_mut().unwrap().config =
                            ReadState::Failed(Failure::new("Invalid configuration"))
                    }
                    3 => {
                        app.snapshot.as_mut().unwrap().config = ReadState::Failed(Failure {
                            kind: FailureKind::Permission,
                            detail: "Access denied".into(),
                            uncertain: false,
                        })
                    }
                    4 => app.snapshot.as_mut().unwrap().node = ReadState::Missing,
                    5 => {
                        let snapshot = app.snapshot.as_mut().unwrap();
                        snapshot.node = ReadState::Missing;
                        snapshot.last_status = ReadState::Missing;
                        snapshot.service_installed = ReadState::Ready(false);
                    }
                    _ => {
                        let snapshot = app.snapshot.as_mut().unwrap();
                        snapshot.node = ReadState::Failed(Failure::new("node_service_not_running"));
                        snapshot.service_installed =
                            ReadState::Failed(Failure::new("Access is denied"));
                    }
                }
                for page in Page::ALL {
                    app.page = page;
                    app.screen = Screen::Page;
                    draw(&mut app, language, width, height);
                }
            }
            for kind in [
                FormKind::Create,
                FormKind::Join,
                FormKind::Invite,
                FormKind::Description,
                FormKind::PrivateNote("linux-builder".into()),
                FormKind::Grant(1),
                FormKind::Cursor,
                FormKind::HubDirectory,
                FormKind::Export("fxw1.secret".into()),
            ] {
                let mut app = fixture();
                app.open_form(kind, "A very long value with 中文 and e\u{301} 🧑‍💻");
                draw(&mut app, language, width, height);
                if let Some(form) = &mut app.form {
                    form.selected = form.fields.len();
                }
                draw(&mut app, language, width, height);
            }
            for screen in [
                Screen::Confirm,
                Screen::Discard,
                Screen::Busy,
                Screen::Receipt,
                Screen::Failed,
            ] {
                let mut app = fixture();
                app.screen = screen;
                app.preview = Some(Action::Revoke {
                    name: "linux-builder".into(),
                });
                app.failure = Some(Failure {
                    kind: FailureKind::Timeout,
                    detail: "The result is not known.".into(),
                    uncertain: true,
                });
                app.receipt = Some(Receipt {
                    invite: Some(format!("fxw1.{}", "abcd".repeat(512))),
                    warnings: vec!["A warning with 中文.".into()],
                    ..Receipt::default()
                });
                draw(&mut app, language, width, height);
            }
        }
    }
}

#[test]
fn the_recommended_action_follows_the_first_broken_step() {
    let mut app = fixture();
    assert_eq!(app.recommended(), None);
    app.snapshot.as_mut().unwrap().service_installed = ReadState::Ready(false);
    assert_eq!(app.recommended(), Some(MenuAction::Install));
    app.snapshot.as_mut().unwrap().node = ReadState::Missing;
    assert_eq!(app.recommended(), Some(MenuAction::Install));
    app.snapshot.as_mut().unwrap().service_installed = ReadState::Ready(true);
    assert_eq!(app.recommended(), Some(MenuAction::Start));
    app.page = Page::Service;
    assert_eq!(app.actions()[0], MenuAction::Start);
    assert!(!app.actions().contains(&MenuAction::Stop));
}

#[test]
fn temporary_world_render_review() {
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("dev-notes/validation/world-tui/render");
    std::fs::create_dir_all(&output).unwrap();
    for (name, language, width, height, page, screen, machines) in [
        (
            "overview-zh",
            Language::ZhCn,
            100,
            30,
            Page::Overview,
            Screen::Page,
            3,
        ),
        (
            "overview-en-80",
            Language::En,
            80,
            24,
            Page::Overview,
            Screen::Page,
            3,
        ),
        (
            "overview-many",
            Language::ZhCn,
            100,
            30,
            Page::Overview,
            Screen::Page,
            9,
        ),
        (
            "overview-narrow",
            Language::ZhCn,
            52,
            12,
            Page::Overview,
            Screen::Page,
            3,
        ),
        (
            "overview-40",
            Language::ZhCn,
            40,
            9,
            Page::Overview,
            Screen::Page,
            3,
        ),
        (
            "welcome-zh",
            Language::ZhCn,
            80,
            24,
            Page::Overview,
            Screen::Page,
            3,
        ),
        (
            "machines-zh",
            Language::ZhCn,
            120,
            36,
            Page::Machines,
            Screen::Page,
            5,
        ),
        (
            "machines-80",
            Language::ZhCn,
            80,
            24,
            Page::Machines,
            Screen::Page,
            5,
        ),
        (
            "service-zh",
            Language::ZhCn,
            100,
            30,
            Page::Service,
            Screen::Page,
            3,
        ),
        (
            "service-down",
            Language::ZhCn,
            100,
            30,
            Page::Service,
            Screen::Page,
            3,
        ),
        (
            "connection-zh",
            Language::ZhCn,
            100,
            30,
            Page::Connection,
            Screen::Page,
            3,
        ),
        (
            "events-zh",
            Language::ZhCn,
            100,
            30,
            Page::Events,
            Screen::Page,
            3,
        ),
        (
            "grants-zh",
            Language::ZhCn,
            100,
            30,
            Page::Grants,
            Screen::Page,
            3,
        ),
        (
            "actions-zh",
            Language::ZhCn,
            80,
            24,
            Page::Overview,
            Screen::Actions,
            3,
        ),
        (
            "overview-de",
            Language::De,
            100,
            30,
            Page::Overview,
            Screen::Page,
            4,
        ),
    ] {
        let mut app = fixture_with(machines);
        app.page = page;
        app.screen = screen;
        match name {
            "welcome-zh" => app.snapshot.as_mut().unwrap().config = ReadState::Missing,
            "service-down" => {
                let snapshot = app.snapshot.as_mut().unwrap();
                snapshot.node = ReadState::Missing;
                snapshot.service_installed = ReadState::Ready(false);
            }
            _ => {}
        }
        let terminal = draw(&mut app, language, width, height);
        let buffer = terminal.backend().buffer();
        let cells=buffer.content.iter().map(|cell|serde_json::json!({"text":cell.symbol(),"fg":format!("{:?}",cell.fg),"bg":format!("{:?}",cell.bg),"reverse":cell.modifier.contains(ratatui::style::Modifier::REVERSED)})).collect::<Vec<_>>();
        std::fs::write(
            output.join(format!("{name}.json")),
            serde_json::to_vec(&serde_json::json!({"width":width,"height":height,"cells":cells}))
                .unwrap(),
        )
        .unwrap();
    }
}
