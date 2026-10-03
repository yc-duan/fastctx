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

pub(super) fn fixture() -> WorldApp {
    fixture_with(3)
}

fn fixture_with(machines: usize) -> WorldApp {
    let paths = ControlPaths::for_home(std::env::temp_dir().join("fastctx-world-render-only"));
    let mut app = WorldApp::new(&paths);
    let now = crate::world::now_rfc3339();
    let status: NodeStatus = serde_json::from_value(serde_json::json!({
        "name":"desktop", "world_id":"world-demo", "hub":["example.test:443"], "hub_key":"public fingerprint", "fingerprint":"public fingerprint", "pid":1234,
        "version":"1.0.0", "started_at":now, "written_at":now,
        "link":{"state":"connected","network":"direct","path":"direct via Ethernet","interface":"Ethernet", "tunnels":[],"tls":"pinned", "connected_since":now,"last_heartbeat_at":now,"last_ack_at":now,"rtt_ms":18,"hub_time_offset_s":0,"last_error":null,"replaced_count":0,"outbox_depth":0},
        "members":machines,"members_online":machines.saturating_sub(machines / 3),"grant_revision":1,"grant_stale":false,"running_calls":1,"engine_hosted":true,"agent_connections":2
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
    let mut config: crate::world::WorldConfig=serde_json::from_value(serde_json::json!({"version":2,"founder":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","admission":{"kind":"founder"},"name":"desktop","world_id":"world-demo","hub":["example.test:443"],"hub_key":"public fingerprint","tls":"pinned","enrolled_at":now})).unwrap();
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
    let hub:crate::world::hub::HubStatus=serde_json::from_value(serde_json::json!({"version":"1.0.0","pid":1000,"written_at":now,"started_at":now,"listen":"0.0.0.0:443","world_id":"world-demo","hub_key":"public fingerprint","tls":"pinned","binding":"direct","members":[{"name":"desktop","state":"online","last_seen":now,"outbox":0,"version":"1.0.0"}],"open_invites":2,"events":30,"bootstrap_used":true,"data_connections":2})).unwrap();
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
    app.rtt_history = (0..39)
        .map(|index| 12 + (index * 7) % 23)
        .chain([18])
        .collect();
    app
}

use crate::world::NetworkMode;

pub(super) fn draw(
    app: &mut WorldApp,
    language: Language,
    width: u16,
    height: u16,
) -> Terminal<TestBackend> {
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
                FormKind::Export("fxw2.secret".into()),
            ] {
                let mut app = fixture();
                app.open_form(kind, "A very long value with 中文 and e\u{301} 🧑‍💻");
                draw(&mut app, language, width, height);
                if let Some(form) = &mut app.form {
                    form.advanced = true;
                    form.focus_submit();
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
                    invite: Some(format!("fxw2.{}", "abcd".repeat(512))),
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

fn review_node(app: &mut WorldApp) -> &mut NodeSnapshot {
    match &mut app.snapshot.as_mut().unwrap().node {
        ReadState::Ready(node) => node,
        _ => panic!("the scene needs a running node"),
    }
}

pub(super) fn export_scene(
    name: &str,
    language: Language,
    width: u16,
    height: u16,
    mut app: WorldApp,
) {
    let scroll = app.scroll;
    let selected = app.selected;
    for (mode_name, mode) in [
        ("truecolor", crate::tui::theme::ColorMode::TrueColor),
        ("ansi16", crate::tui::theme::ColorMode::Ansi16),
        ("mono", crate::tui::theme::ColorMode::Monochrome),
    ] {
        let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("dev-notes/validation/world-tui/render-redesign")
            .join(mode_name);
        std::fs::create_dir_all(&output).unwrap();
        app.scroll = scroll;
        app.selected = selected;
        let terminal =
            crate::tui::theme::with_test_mode(mode, || draw(&mut app, language, width, height));
        write_cells(
            &output.join(format!("{name}.json")),
            terminal.backend().buffer(),
        );
    }
}

/// Writes every World scene as a cell dump for the render review, in all three colour depths.
///
/// Look is judged from rendered pixels, never from code or an ASCII sketch, so this exists to
/// feed `dev-notes/validation/world-tui/render_cells.py`, which turns the dumps into PNGs on a
/// real monospace grid. It asserts nothing and costs about a minute, so it stays out of the
/// ordinary suite:
///
/// ```text
/// cargo test --lib tui::world::tests::export_world_render_scenes -- --ignored --exact
/// python dev-notes/validation/world-tui/render_cells.py dev-notes/validation/world-tui/render-redesign/truecolor
/// ```
#[test]
#[ignore = "developer tool: exports render-review scenes, asserts nothing"]
fn export_world_render_scenes() {
    for (name, language, width, height, page, machines) in [
        ("overview-zh", Language::ZhCn, 100, 30, Page::Overview, 3),
        ("overview-en-80", Language::En, 80, 24, Page::Overview, 3),
        ("overview-many", Language::ZhCn, 100, 30, Page::Overview, 9),
        (
            "overview-single",
            Language::ZhCn,
            100,
            30,
            Page::Overview,
            1,
        ),
        ("overview-narrow", Language::ZhCn, 52, 12, Page::Overview, 3),
        ("overview-40", Language::ZhCn, 40, 9, Page::Overview, 3),
        ("overview-de", Language::De, 100, 30, Page::Overview, 4),
        ("machines-zh", Language::ZhCn, 120, 36, Page::Machines, 5),
        ("machines-en", Language::En, 120, 36, Page::Machines, 3),
        ("machines-80", Language::ZhCn, 80, 24, Page::Machines, 5),
        ("machines-40", Language::ZhCn, 40, 9, Page::Machines, 5),
        ("service-zh", Language::ZhCn, 100, 30, Page::Service, 3),
        ("service-40", Language::ZhCn, 40, 9, Page::Service, 3),
        ("service-de", Language::De, 80, 24, Page::Service, 3),
        (
            "connection-zh",
            Language::ZhCn,
            100,
            30,
            Page::Connection,
            3,
        ),
        ("connection-40", Language::ZhCn, 40, 9, Page::Connection, 3),
        ("events-zh", Language::ZhCn, 100, 30, Page::Events, 3),
        ("grants-zh", Language::ZhCn, 100, 30, Page::Grants, 3),
        ("grants-40", Language::ZhCn, 40, 9, Page::Grants, 3),
        ("hub-zh", Language::ZhCn, 100, 30, Page::Hub, 3),
    ] {
        let mut app = fixture_with(machines);
        app.page = page;
        export_scene(name, language, width, height, app);
    }
    for page in Page::ALL {
        for state in [
            "loading",
            "empty",
            "error",
            "permission",
            "unregistered",
            "stopped",
        ] {
            let mut app = fixture();
            app.page = page;
            match state {
                "loading" => {
                    app.snapshot = None;
                    app.events.clear();
                    app.events_loaded = false;
                }
                "unregistered" => app.snapshot.as_mut().unwrap().config = ReadState::Missing,
                "stopped" => app.snapshot.as_mut().unwrap().node = ReadState::Missing,
                "error" | "permission" => {
                    let error = Failure {
                        kind: if state == "permission" {
                            FailureKind::Permission
                        } else {
                            FailureKind::Invalid
                        },
                        detail: "Cannot read C:/World/state.json: access denied or invalid data."
                            .into(),
                        uncertain: false,
                    };
                    let snapshot = app.snapshot.as_mut().unwrap();
                    snapshot.node = ReadState::Failed(error.clone());
                    snapshot.hub = ReadState::Failed(error.clone());
                    if page == Page::Events {
                        app.events.clear();
                        app.event_error = Some(error);
                    }
                }
                "empty" => match page {
                    Page::Overview | Page::Machines => {
                        review_node(&mut app).nodes = ReadState::Ready(Vec::new())
                    }
                    Page::Grants => {
                        review_node(&mut app).grants.grants.clear();
                        review_node(&mut app).grants.revision = 0;
                    }
                    Page::Events => app.events.clear(),
                    Page::Connection => app.rtt_history.clear(),
                    Page::Hub => app.snapshot.as_mut().unwrap().hub = ReadState::Missing,
                    Page::Service => {
                        app.snapshot.as_mut().unwrap().service_installed = ReadState::Ready(false)
                    }
                    Page::Towns => {
                        app.towns.census = Some(Ok(super::towns_tests::census(Vec::new())))
                    }
                },
                _ => unreachable!(),
            }
            export_scene(
                &format!("boundary-{}-{state}", page.index() + 1),
                Language::ZhCn,
                40,
                9,
                app,
            );
        }
    }
    for (name, size) in [
        ("welcome-zh", (80, 24)),
        ("welcome-40", (40, 9)),
        // Tall enough for the sketch and the three steps together.
        ("welcome-tall", (100, 32)),
    ] {
        let mut app = fixture();
        app.snapshot.as_mut().unwrap().config = ReadState::Missing;
        export_scene(name, Language::ZhCn, size.0, size.1, app);
    }
    for (name, page) in [
        ("actions-zh", Page::Overview),
        ("service-actions", Page::Service),
    ] {
        let mut app = fixture();
        app.page = page;
        app.screen = Screen::Actions;
        export_scene(name, Language::ZhCn, 80, 24, app);
    }
    let mut app = fixture_with(33);
    app.select(32);
    export_scene("overview-tail", Language::ZhCn, 100, 30, app);
    let mut app = fixture_with(33);
    app.select(32);
    export_scene("overview-tail-40", Language::ZhCn, 40, 9, app);
    let mut app = fixture_with(3);
    if let ReadState::Ready(nodes) = &mut review_node(&mut app).nodes {
        nodes[0].name = "production-workstation-0123456789".into();
        nodes[1].name = "hangzhou-build-machine-012345678".into();
        nodes[1].arch = "loongarch64".into();
        nodes[2].state = "revoked".into();
        nodes[0].private_note =
            Some("我的主力工作站 · 多语言说明 e\u{301} · 日本語 · 한국어".into());
    }
    export_scene("overview-long", Language::ZhCn, 100, 30, app);
    let mut app = fixture();
    app.load_error = Some(Failure::new("The local snapshot request timed out."));
    app.page = Page::Machines;
    export_scene("refresh-failed", Language::ZhCn, 80, 24, app);
    // The two screens every page falls back to: the focused object's full record, and the keys.
    for (name, screen, page) in [
        ("detail-machine", Screen::Detail, Page::Machines),
        ("help-overview", Screen::Help, Page::Overview),
        ("help-connection", Screen::Help, Page::Connection),
    ] {
        let mut app = fixture();
        app.page = page;
        app.screen = screen;
        export_scene(name, Language::ZhCn, 100, 30, app);
    }
    let mut app = fixture();
    review_node(&mut app).status.written_at = "2020-01-01T00:00:00Z".into();
    app.page = Page::Service;
    export_scene("service-stale", Language::ZhCn, 100, 30, app);
    for (name, installed) in [("service-down", false), ("service-start", true)] {
        let mut app = fixture();
        app.page = Page::Service;
        app.snapshot.as_mut().unwrap().node = ReadState::Missing;
        app.snapshot.as_mut().unwrap().service_installed = ReadState::Ready(installed);
        export_scene(name, Language::ZhCn, 100, 30, app);
    }
    for (name, reason) in [
        ("connection-revoked", "revoked: removed by owner"),
        ("connection-protocol", "protocol_mismatch"),
        ("connection-identity", "hub_identity_mismatch"),
    ] {
        let mut app = fixture();
        app.page = Page::Connection;
        review_node(&mut app).status.link.state = crate::world::client::LinkState::Stopped {
            reason: reason.into(),
            until: None,
        };
        export_scene(name, Language::ZhCn, 100, 30, app);
    }
    let mut app = fixture();
    app.page = Page::Connection;
    review_node(&mut app).status.link.state = crate::world::client::LinkState::Reconnecting {
        attempt: 7,
        next_attempt_at: crate::world::now_rfc3339(),
    };
    review_node(&mut app).status.link.last_error =
        Some("connect failed: connection refused".into());
    export_scene("connection-retrying", Language::ZhCn, 80, 24, app);
    let mut app = fixture();
    app.page = Page::Events;
    app.event_error = Some(Failure::new("The hub did not answer."));
    app.event_gap = true;
    export_scene("events-retained-error", Language::ZhCn, 80, 24, app);
    for (name, kind) in [
        ("create", FormKind::Create),
        ("join", FormKind::Join),
        ("invite", FormKind::Invite),
        ("description", FormKind::Description),
        ("note", FormKind::PrivateNote("machine-1".into())),
        ("grant", FormKind::Grant(1)),
        ("cursor", FormKind::Cursor),
        ("hub-directory", FormKind::HubDirectory),
        ("export", FormKind::Export("fxw2.example-only".into())),
    ] {
        for (suffix, language, width, height) in [
            ("zh", Language::ZhCn, 80, 24),
            ("40", Language::ZhCn, 40, 9),
            ("de", Language::De, 80, 24),
            // Wide enough for the guide pane, and tall enough for every field to explain itself.
            ("wide", Language::ZhCn, 120, 36),
            ("tall", Language::ZhCn, 84, 36),
        ] {
            let mut app = fixture();
            app.open_form(kind.clone(), "");
            if let Some(form) = &mut app.form {
                if matches!(form.kind, FormKind::Description | FormKind::PrivateNote(_)) {
                    form.fields[0]
                        .input
                        .replace("第一行 · e\u{301}\nSecond line · 日本語".into());
                }
                if matches!(form.kind, FormKind::Export(_)) {
                    form.fields[0]
                        .input
                        .replace("C:/Users/Ada/Documents/邀请/world-invite.txt".into());
                }
            }
            export_scene(
                &format!("form-{name}-{suffix}"),
                language,
                width,
                height,
                app,
            );
        }
    }
    for (name, selected) in [("form-create-submit", true), ("form-create-invalid", false)] {
        let mut app = fixture();
        app.open_form(FormKind::Create, "bad name");
        let form = app.form.as_mut().unwrap();
        if selected {
            form.focus_submit();
        } else {
            assert!(form.submit().is_err());
        }
        export_scene(name, Language::ZhCn, 40, 9, app);
    }
    let mut app = fixture();
    app.open_form(FormKind::Grant(1), "");
    let form = app.form.as_mut().unwrap();
    form.fields[0].input.replace("invalid principal".into());
    form.fields[1].input.replace("all".into());
    form.fields[2].input.replace("grep".into());
    assert!(form.submit().is_err());
    export_scene("form-grant-invalid", Language::ZhCn, 80, 24, app);
    for (name, action) in [
        (
            "revoke",
            Action::Revoke {
                name: "hangzhou-build-machine-012345678".into(),
            },
        ),
        (
            "stop",
            Action::Service {
                action: ServiceAction::Stop,
            },
        ),
        ("leave", Action::Leave),
        (
            "delete-grant",
            Action::DeleteGrant {
                id: "g-a12345".into(),
                expected_revision: 1,
            },
        ),
    ] {
        for (suffix, width, height) in [("zh", 80, 24), ("40", 40, 9)] {
            let mut app = fixture();
            app.preview = Some(action.clone());
            app.screen = Screen::Confirm;
            export_scene(
                &format!("confirm-{name}-{suffix}"),
                Language::ZhCn,
                width,
                height,
                app,
            );
        }
    }
    for (name, screen) in [
        ("busy", Screen::Busy),
        ("failed", Screen::Failed),
        ("discard", Screen::Discard),
    ] {
        let mut app = fixture();
        app.screen = screen;
        app.active = Some(Action::Service {
            action: ServiceAction::Restart,
        });
        app.failure = Some(Failure {
            kind: FailureKind::Timeout,
            detail: "The action timed out.".into(),
            uncertain: true,
        });
        export_scene(name, Language::ZhCn, 40, 9, app);
    }
    for (name, warning) in [("receipt-service", false), ("receipt-partial", true)] {
        let mut app = fixture();
        app.screen = Screen::Receipt;
        app.receipt_action = Some(MenuAction::Start);
        app.receipt = Some(Receipt {
            facts: vec![(
                "service".into(),
                "Started the node for this HOME and build.".into(),
            )],
            warnings: if warning {
                vec!["The service command completed, but the node did not answer.".into()]
            } else {
                Vec::new()
            },
            ..Receipt::default()
        });
        export_scene(name, Language::ZhCn, 80, 24, app);
    }
    let mut app = fixture();
    app.screen = Screen::Receipt;
    app.receipt_action = Some(MenuAction::Invite);
    app.receipt = Some(Receipt {
        invite: Some(format!("fxw2.{}", "example-public-fixture-".repeat(30))),
        ..Receipt::default()
    });
    export_scene("receipt-invite", Language::ZhCn, 80, 24, app);
    for language in ALL_LANGUAGES {
        let mut app = fixture();
        app.page = Page::Service;
        export_scene(&format!("language-{language:?}"), language, 80, 24, app);
    }
    // The main terminal, for judging whether the World pages and the rest read as one product.
    let temp = std::env::temp_dir().join("fastctx-world-render-only");
    std::fs::create_dir_all(&temp).unwrap();
    let paths = ControlPaths::for_home(&temp);
    let executable = temp.join("fastctx-fixture.exe");
    std::fs::write(&executable, b"fixture").unwrap();
    for (name, screen, selected) in [
        ("main-menu", crate::tui::app::Screen::Main, 1),
        ("main-about", crate::tui::app::Screen::About, 0),
        ("main-agents", crate::tui::app::Screen::AgentList, 0),
        ("main-config", crate::tui::app::Screen::Config, 0),
        ("main-jobs", crate::tui::app::Screen::Jobs, 0),
        ("main-update", crate::tui::app::Screen::Update, 0),
        ("main-status", crate::tui::app::Screen::Status, 0),
        (
            "main-language",
            crate::tui::app::Screen::Language { first_run: false },
            0,
        ),
        // The two card-over-page shapes: a confirmation and an inline editor.
        ("main-confirm", crate::tui::app::Screen::ApplyConfirm, 1),
        ("main-cpu-edit", crate::tui::app::Screen::ConfigCpuEdit, 0),
    ] {
        let mut app = crate::tui::app::App::for_test(paths.clone(), executable.clone());
        app.settings.language = Some("zh-cn".to_string());
        app.language = Language::ZhCn;
        app.screen = screen;
        app.selected = selected;
        for (mode_name, mode) in [
            ("truecolor", crate::tui::theme::ColorMode::TrueColor),
            ("ansi16", crate::tui::theme::ColorMode::Ansi16),
            ("mono", crate::tui::theme::ColorMode::Monochrome),
        ] {
            let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("dev-notes/validation/world-tui/render-redesign")
                .join(mode_name);
            std::fs::create_dir_all(&output).unwrap();
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            crate::tui::theme::with_test_mode(mode, || {
                terminal
                    .draw(|frame| crate::tui::view::render(frame, &mut app))
                    .unwrap();
            });
            write_cells(
                &output.join(format!("{name}.json")),
                terminal.backend().buffer(),
            );
        }
    }
}

fn write_cells(path: &std::path::Path, buffer: &ratatui::buffer::Buffer) {
    let cells = buffer
        .content
        .iter()
        .map(|cell| {
            serde_json::json!({
                "text":cell.symbol(),"fg":format!("{:?}",cell.fg),"bg":format!("{:?}",cell.bg),
                "reverse":cell.modifier.contains(ratatui::style::Modifier::REVERSED),
                "bold":cell.modifier.contains(ratatui::style::Modifier::BOLD),
                "underline":cell.modifier.contains(ratatui::style::Modifier::UNDERLINED)
            })
        })
        .collect::<Vec<_>>();
    std::fs::write(
        path,
        serde_json::to_vec(&serde_json::json!({
            "width":buffer.area.width,"height":buffer.area.height,"cells":cells
        }))
        .unwrap(),
    )
    .unwrap();
}
