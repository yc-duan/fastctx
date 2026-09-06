//! Cross-language, cross-size rendering health for the World control surface.

use super::*;
use crate::control::i18n::{ALL_LANGUAGES, Language};
use crate::world::client::Inventory;
use crate::world::control::{FailureKind, NodeSnapshot};
use crate::world::grant::{Grant, GrantEntry, GrantSnapshot};
use crate::world::node::admin::NodeStatus;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn fixture() -> WorldApp {
    let paths = ControlPaths::for_home(std::env::temp_dir().join("fastctx-world-render-only"));
    let mut app = WorldApp::new(&paths);
    let now = crate::world::now_rfc3339();
    let status: NodeStatus = serde_json::from_value(serde_json::json!({
        "name":"desktop", "world_id":"world-demo", "hub":["example.test:443"], "hub_key":"public fingerprint", "fingerprint":"public fingerprint", "pid":1234,
        "version":"1.0.0", "started_at":now, "written_at":now,
        "link":{"state":"connected","network":"direct","path":"direct via Ethernet","interface":"Ethernet", "tunnels":[],"tls":"pinned", "connected_since":now,"last_heartbeat_at":now,"last_ack_at":now,"rtt_ms":18,"hub_time_offset_s":0,"last_error":null,"replaced_count":0,"key_epoch":2,"outbox_depth":0},
        "members":2,"members_online":2,"grant_revision":1,"grant_stale":false,"running_calls":1,"engine_hosted":true,"agent_connections":2
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
    let mut second = first.clone();
    second.name = "linux-builder".into();
    second.os = "linux".into();
    second.is_self = false;
    second.private_note = Some("我的服务器".into());
    let mut config: crate::world::WorldConfig=serde_json::from_value(serde_json::json!({"version":1,"name":"desktop","world_id":"world-demo","hub":["example.test:443"],"hub_key":"public fingerprint","tls":"pinned","enrolled_at":now})).unwrap();
    config.network = NetworkMode::Direct;
    let node = NodeSnapshot {
        status: status.clone(),
        nodes: ReadState::Ready(vec![first, second]),
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
    app.events = vec![Event {
        seq: 1,
        at: now,
        subject: "desktop".into(),
        kind: "node.online".into(),
        facts: Default::default(),
    }];
    app.event_cursor = 1;
    app.event_latest = 1;
    app.events_loaded = true;
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
                let mut app = fixture();
                app.page = page;
                draw(&mut app, language, width, height);
                app.screen = Screen::Detail;
                draw(&mut app, language, width, height);
                app.screen = Screen::Actions;
                draw(&mut app, language, width, height);
            }
            for state in 0..6 {
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
                    _ => {
                        let snapshot = app.snapshot.as_mut().unwrap();
                        snapshot.node = ReadState::Missing;
                        snapshot.last_status = ReadState::Missing;
                    }
                }
                draw(&mut app, language, width, height);
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
fn temporary_world_render_review() {
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("dev-notes/validation/world-tui/render");
    std::fs::create_dir_all(&output).unwrap();
    for (name, language, width, height, screen) in [
        ("welcome-zh", Language::ZhCn, 80, 24, Screen::Page),
        ("welcome-40", Language::ZhCn, 40, 9, Screen::Page),
        ("create-40", Language::ZhCn, 40, 9, Screen::Form),
        ("create-de", Language::De, 80, 24, Screen::Form),
        ("revoke-zh", Language::ZhCn, 80, 24, Screen::Confirm),
        ("machines-zh", Language::ZhCn, 120, 36, Screen::Detail),
    ] {
        let mut app = fixture();
        app.screen = screen;
        match screen {
            Screen::Page => app.snapshot.as_mut().unwrap().config = ReadState::Missing,
            Screen::Form => app.open_form(FormKind::Create, "desktop"),
            Screen::Confirm => {
                app.preview = Some(Action::Revoke {
                    name: "linux-builder".into(),
                })
            }
            Screen::Detail => app.page = Page::Machines,
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
