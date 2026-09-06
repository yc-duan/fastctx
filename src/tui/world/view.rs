//! Adaptive World views: one keyboard model, with a detail pane when space allows it.

use super::widgets::{
    card, choice_style, intro_list, list, pair, plain, safe, scroll, warn, wrapped,
};
use super::{Page, Screen, WorldApp, i18n};
use crate::control::i18n::Language;
use crate::tui::text_input::InputError;
use crate::tui::theme;
use crate::world::client::{LinkState, NodeView};
use crate::world::control::{Action, Failure, FailureKind, ReadState, ServiceAction};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
type Lines = Vec<Line<'static>>;

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut WorldApp, language: Language, area: Rect) {
    let m = i18n::messages(language);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);
    let working = app
        .worker
        .as_ref()
        .filter(|worker| !worker.read_only)
        .map(|worker| {
            format!(
                "{} · {} {}s",
                app.active
                    .as_ref()
                    .map(|action| action_name(action, m))
                    .unwrap_or(m.actions),
                m.busy,
                worker.started.elapsed().as_secs()
            )
        })
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            if working.is_empty() {
                format!(" FastCtx / {} / {}", m.title, app.page.title(m))
            } else {
                format!(" {working}")
            },
            Style::default()
                .fg(if working.is_empty() {
                    theme::accent()
                } else {
                    theme::warning()
                })
                .add_modifier(Modifier::BOLD),
        )]))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme::border())),
        ),
        rows[0],
    );
    match app.screen {
        Screen::Page => render_page(frame, app, m, rows[1]),
        Screen::Detail => {
            let lines = detail(app, m);
            scroll(frame, app, rows[1], app.page.title(m), lines);
        }
        Screen::Actions => {
            let items = app
                .actions()
                .iter()
                .map(|action| action.label(m).to_string())
                .collect::<Vec<_>>();
            list(frame, rows[1], m.actions, items, app.menu_selected);
        }
        Screen::Form => render_form(frame, app, m, rows[1]),
        Screen::Confirm | Screen::Discard => render_confirmation(frame, app, m, rows[1]),
        Screen::Busy => {
            let lines = vec![
                pair(
                    m.actions,
                    app.active
                        .as_ref()
                        .map(|action| action_name(action, m))
                        .unwrap_or(m.actions),
                ),
                plain(m.busy_body),
                plain(""),
                warn(m.running_exit),
            ];
            scroll(frame, app, rows[1], m.busy, lines);
        }
        Screen::Failed => {
            let lines = app
                .failure
                .as_ref()
                .map(|failure| failure_lines(failure, m))
                .unwrap_or_else(|| vec![plain(m.failed)]);
            scroll(frame, app, rows[1], m.failed, lines);
        }
        Screen::Receipt => render_receipt(frame, app, m, rows[1]),
        Screen::Help => {
            let lines = help_lines(app, m);
            scroll(frame, app, rows[1], m.facts, lines);
        }
    }
    let footer = if area.width < 72 {
        match app.screen {
            Screen::Form => vec![
                plain(format!("Tab  F1 {}  Ctrl-S {}", m.facts, m.save)),
                plain(format!("Ctrl-A / Ctrl-U   Esc {}", m.back)),
            ],
            Screen::Confirm | Screen::Discard => vec![
                plain(format!("← →  Enter   Esc {}", m.cancel)),
                plain("↑ ↓  PgUp / PgDn"),
            ],
            Screen::Help => vec![
                plain(format!("↑ ↓  PgUp / PgDn   Esc {}", m.back)),
                plain(format!("F1 {}", m.back)),
            ],
            Screen::Detail => vec![
                plain(format!("↑ ↓  PgUp / PgDn   Esc {}", m.back)),
                plain(format!("a {}  F1 {}", m.actions, m.facts)),
            ],
            Screen::Receipt => vec![
                plain(format!("Enter   Esc {}", m.back)),
                plain(
                    if app
                        .receipt
                        .as_ref()
                        .is_some_and(|receipt| receipt.invite.is_some())
                    {
                        format!("e {}", m.export)
                    } else {
                        String::new()
                    },
                ),
            ],
            Screen::Failed | Screen::Busy => vec![
                plain(format!("Enter / Esc {}", m.back)),
                plain("↑ ↓  PgUp / PgDn"),
            ],
            _ => vec![
                plain(format!(
                    "← →  {}/7   a {}  F1",
                    app.page.index() + 1,
                    m.actions
                )),
                plain(format!("↑ ↓  Enter   Esc {}", m.back)),
            ],
        }
    } else {
        match app.screen {
            Screen::Form => vec![plain(m.form_keys), plain(m.edit_keys)],
            Screen::Confirm | Screen::Discard => vec![plain(m.confirm_keys), plain(m.scroll_keys)],
            Screen::Receipt
                if app
                    .receipt
                    .as_ref()
                    .is_some_and(|receipt| receipt.invite.is_some()) =>
            {
                vec![plain(format!("e  {}", m.export)), plain(m.receipt_keys)]
            }
            Screen::Help => vec![plain(m.scroll_keys), plain(format!("F1 {}", m.back))],
            Screen::Detail => vec![
                plain(format!("a  {}   F1 {}", m.actions, m.facts)),
                plain(m.scroll_keys),
            ],
            Screen::Failed => vec![plain(format!("Enter {}", m.back)), plain(m.scroll_keys)],
            Screen::Busy | Screen::Receipt => vec![plain(m.receipt_keys)],
            _ => vec![
                plain(format!("{}   F1 {}", m.shortcuts, m.facts)),
                plain(format!(
                    "{}/7  ·  r {}",
                    app.page.index() + 1,
                    if app.page == Page::Hub {
                        m.inspect
                    } else {
                        m.refresh
                    }
                )),
            ],
        }
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(theme::muted())),
        rows[2],
    );
}

fn render_page(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    app.viewport_rows = area.height.saturating_sub(2).max(1) as usize;
    if app.page == Page::Hub {
        render_hub(frame, app, m, area);
        return;
    }
    let Some(snapshot) = &app.snapshot else {
        if let Some(failure) = &app.load_error {
            scroll(frame, app, area, m.error, failure_lines(failure, m));
        } else {
            scroll(frame, app, area, m.title, vec![plain(m.loading)]);
        }
        return;
    };
    match &snapshot.config {
        ReadState::Missing => {
            let items = app
                .actions()
                .iter()
                .map(|action| action.label(m).to_string())
                .collect();
            intro_list(
                frame,
                area,
                m.welcome,
                vec![plain(m.welcome_body)],
                items,
                app.selected[app.page.index()],
            );
            return;
        }
        ReadState::Failed(failure) => {
            let lines = failure_lines(failure, m);
            scroll(frame, app, area, m.error, lines);
            return;
        }
        ReadState::Ready(_) => {}
    }
    match app.page {
        Page::Overview => {
            let mut info = overview(app, m);
            if let Some(error) = &app.load_error {
                info.extend(failure_lines(error, m));
            }
            let items = app
                .actions()
                .iter()
                .map(|action| action.label(m).to_string())
                .collect();
            intro_list(
                frame,
                area,
                m.overview,
                info,
                items,
                app.selected[app.page.index()],
            );
        }
        Page::Service => {
            let mut lines = overview(app, m);
            lines.push(plain(m.service_warning));
            let items = app
                .actions()
                .iter()
                .map(|action| action.label(m).to_string())
                .collect();
            intro_list(
                frame,
                area,
                m.service,
                lines,
                items,
                app.selected[app.page.index()],
            );
        }
        Page::Machines => {
            if let Some(lines) = node_error(app, m, true) {
                scroll(frame, app, area, m.machines, lines);
                return;
            }
            let nodes = app.nodes();
            if nodes.is_empty() {
                scroll(frame, app, area, m.machines, vec![plain(m.empty)]);
                return;
            }
            let items = nodes
                .iter()
                .map(|node| {
                    format!(
                        "{}{} · {} · {}/{}{}",
                        node.name,
                        if node.is_self { " ●" } else { "" },
                        state_name(&node.state, m),
                        node.os,
                        node.arch,
                        node.effective_note()
                            .map(|note| format!("  {}", safe(note)))
                            .unwrap_or_default()
                    )
                })
                .collect();
            if area.width >= 100 && area.height >= 18 {
                let panes = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
                    .split(area);
                list(
                    frame,
                    panes[0],
                    m.machines,
                    items,
                    app.selected[Page::Machines.index()],
                );
                let lines = app
                    .selected_node()
                    .map(|node| node_details(node, m))
                    .unwrap_or_default();
                scroll(frame, app, panes[1], m.facts, lines);
            } else {
                list(
                    frame,
                    area,
                    m.machines,
                    items,
                    app.selected[Page::Machines.index()],
                );
            }
        }
        Page::Grants => {
            if let Some(lines) = node_error(app, m, false) {
                scroll(frame, app, area, m.grants, lines);
                return;
            }
            let Some(node) = app
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.node.get())
            else {
                return;
            };
            if node.grants.grants.is_empty() {
                let lines = vec![
                    pair(m.grant_revision, node.grants.revision),
                    plain(if node.grants.revision == 0 {
                        m.grants_default
                    } else {
                        m.grants_empty
                    }),
                    plain(""),
                    plain(format!("a  →  {}", m.add_grant)),
                ];
                scroll(frame, app, area, m.grants, lines);
            } else {
                let items = node
                    .grants
                    .grants
                    .iter()
                    .map(|entry| {
                        format!(
                            "{}  {} → {} · {}",
                            entry.id,
                            safe(&entry.grant.principal),
                            entry.grant.nodes.join(", "),
                            entry.grant.verbs.join(", ")
                        )
                    })
                    .collect();
                intro_list(
                    frame,
                    area,
                    m.grants,
                    vec![
                        pair(m.grant_revision, node.grants.revision),
                        plain(m.grants_help),
                    ],
                    items,
                    app.selected[Page::Grants.index()],
                );
            }
        }
        Page::Health => {
            let lines = health(app, m);
            scroll(frame, app, area, m.health, lines);
        }
        Page::Events => {
            if let Some(error) = &app.event_error {
                let mut lines = failure_lines(error, m);
                lines.push(pair(m.cursor, app.event_cursor));
                lines.push(plain(format!("a  →  {}", m.next_events)));
                if app.events.is_empty() {
                    scroll(frame, app, area, m.events, lines);
                    return;
                }
            }
            let mut intro = vec![
                pair(m.cursor, app.event_cursor),
                plain(if app.event_cursor < app.event_latest {
                    m.more_events
                } else if app.events_loaded {
                    m.caught_up
                } else {
                    m.loading
                }),
            ];
            if app.event_gap {
                intro.push(warn(m.event_gap));
            }
            if let Some(error) = &app.event_error {
                intro.push(warn(&error.detail));
            }
            if app.events.is_empty() {
                scroll(frame, app, area, m.events, intro);
            } else {
                let selected = app.selected[Page::Events.index()];
                let start = selected.saturating_sub(area.height as usize);
                let items = app
                    .events
                    .iter()
                    .skip(start)
                    .take(area.height.max(1) as usize * 3)
                    .map(|event| {
                        format!(
                            "#{}  {}  {}  {}",
                            event.seq,
                            event.at,
                            safe(&event.kind),
                            safe(&event.subject)
                        )
                    })
                    .collect();
                intro_list(
                    frame,
                    area,
                    &format!("{}  {}/{}", m.events, selected + 1, app.events.len()),
                    intro,
                    items,
                    selected - start,
                );
            }
        }
        Page::Hub => unreachable!(),
    }
}

fn overview(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    let Some(snapshot) = &app.snapshot else {
        return vec![plain(m.loading)];
    };
    let mut lines = Vec::new();
    if let Some(config) = snapshot.config.get() {
        lines.extend([
            pair(m.this_machine, &config.name),
            pair(m.title, &config.world_id),
            pair(m.hub_address, config.hub.join(", ")),
        ]);
    }
    match &snapshot.service_installed {
        ReadState::Ready(installed) => lines.push(pair(
            m.service,
            if *installed {
                m.installed
            } else {
                m.not_installed
            },
        )),
        ReadState::Failed(error) => lines.push(warn(format!("{}: {}", m.service, error.detail))),
        ReadState::Missing => lines.push(pair(m.service, m.unknown)),
    }
    let status = snapshot
        .node
        .get()
        .map(|node| &node.status)
        .or_else(|| snapshot.last_status.get());
    if let Some(status) = status {
        if snapshot.node.get().is_none() || !crate::world::doctor::status_is_fresh(status) {
            lines.push(warn(m.stale));
        }
        lines.extend([
            pair(m.health, link_name(&status.link.state, m)),
            pair(
                m.machines,
                format!(
                    "{} / {} {}",
                    status.members_online, status.members, m.online
                ),
            ),
            pair(m.engine, yes(status.engine_hosted, m)),
            pair(
                m.sessions,
                status
                    .agent_connections
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| m.unknown.into()),
            ),
            pair(m.running_calls, status.running_calls),
            pair(m.observed, &status.written_at),
        ]);
    } else {
        lines.push(warn(m.unavailable));
        lines.push(plain(m.unavailable_body));
    }
    if let ReadState::Failed(error) = &snapshot.node {
        lines.push(warn(&error.detail));
    }
    if let ReadState::Failed(error) = &snapshot.last_status {
        lines.push(warn(&error.detail));
    }
    lines
}

fn node_error(app: &WorldApp, m: &i18n::WorldMessages, check_nodes: bool) -> Option<Lines> {
    let snapshot = app.snapshot.as_ref()?;
    match &snapshot.node {
        ReadState::Ready(node) => match &node.nodes {
            ReadState::Failed(error) if check_nodes => Some(failure_lines(error, m)),
            _ => None,
        },
        ReadState::Failed(error) => {
            let mut lines = failure_lines(error, m);
            lines.push(plain(m.unavailable_body));
            Some(lines)
        }
        ReadState::Missing => Some(vec![plain(m.unavailable), plain(m.unavailable_body)]),
    }
}

fn node_details(node: &NodeView, m: &i18n::WorldMessages) -> Lines {
    let mut lines = vec![
        pair(m.name, &node.name),
        pair(m.health, state_name(&node.state, m)),
        pair(m.system, format!("{}/{}", node.os, node.arch)),
        pair(m.last_seen, &node.last_seen),
        pair(m.version, &node.version),
        pair(m.tags, node.tags.join(", ")),
        pair(m.effective_note, node.effective_note().unwrap_or(m.none)),
    ];
    lines.push(pair(
        m.private_note,
        node.private_note.as_deref().unwrap_or(m.none),
    ));
    lines.push(plain(m.note_help));
    lines.push(pair(
        m.description,
        node.inventory
            .as_ref()
            .map(|inventory| inventory.description.as_str())
            .filter(|text| !text.is_empty())
            .unwrap_or(m.none),
    ));
    lines.push(plain(m.description_help));
    if let Some(inv) = &node.inventory {
        if let Some(activity) = &inv.activity {
            if node.state == "online" && activity.is_fresh() {
                lines.extend([
                    pair(m.engine, yes(activity.engine_hosted, m)),
                    pair(m.sessions, activity.agent_connections),
                    pair(
                        m.running_calls,
                        format!("{} + {}", activity.local_requests, activity.remote_calls),
                    ),
                ]);
            } else {
                lines.extend([
                    warn(m.stale),
                    pair(m.engine, m.unknown),
                    pair(m.sessions, m.unknown),
                    pair(m.running_calls, m.unknown),
                ]);
            }
            lines.push(pair(m.observed, &activity.observed_at));
        } else {
            lines.push(pair(m.sessions, m.unknown));
        }
        lines.extend([
            plain(""),
            pair(m.host, &inv.hostname),
            pair(m.home, &inv.home),
            pair(m.user, &inv.user),
            pair("CPU", inv.cpus),
            pair(m.memory, format!("{} GiB", inv.memory_gb)),
            pair(
                m.shell,
                format!(
                    "{} · {} · {}",
                    inv.shell.kind,
                    inv.shell.path,
                    yes(inv.shell.login_ok, m)
                ),
            ),
            pair(m.capabilities, inv.capabilities.join(", ")),
            pair(m.addresses, inv.addresses.join(", ")),
            pair(m.inventory_time, &inv.collected_at),
        ]);
        if let Some(error) = &inv.shell.error {
            lines.push(warn(error));
        }
        for disk in &inv.disks {
            lines.push(pair(
                m.disks,
                format!("{}  {} / {} GiB", disk.mount, disk.free_gb, disk.total_gb),
            ));
        }
        for gpu in &inv.gpus {
            lines.push(pair(
                m.gpus,
                format!("{}  {} GiB", gpu.model, gpu.memory_gb),
            ));
        }
        if let Some(wsl) = inv.wsl2 {
            lines.push(pair("WSL2", yes(wsl, m)));
        }
    } else {
        lines.push(pair(m.hardware, m.unknown));
    }
    lines.extend([
        pair(m.network, node.network.as_deref().unwrap_or(m.unknown)),
        pair(m.tls, node.tls.as_deref().unwrap_or(m.unknown)),
        pair(
            m.rtt,
            node.hub_rtt_ms
                .map(|rtt| format!("{rtt} ms"))
                .unwrap_or_else(|| m.unknown.into()),
        ),
    ]);
    lines
}

fn health(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    let mut lines = overview(app, m);
    if let Some(node) = app
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.node.get())
    {
        for (name, reason) in &node.unverified {
            lines.push(warn(format!(
                "{} ({}) {}: {}",
                m.machines, m.unknown, name, reason
            )));
        }
    }
    let status = app.snapshot.as_ref().and_then(|snapshot| {
        snapshot
            .node
            .get()
            .map(|node| &node.status)
            .or_else(|| snapshot.last_status.get())
    });
    if let Some(status) = status {
        let link = &status.link;
        lines.extend([
            plain(""),
            pair(m.network, link.path.as_deref().unwrap_or(m.unknown)),
            pair(m.interface, link.interface.as_deref().unwrap_or(m.none)),
            pair("TUN", link.tunnels.join(", ")),
            pair(m.tls, link.tls.as_str()),
            pair(
                m.heartbeat,
                link.last_heartbeat_at.as_deref().unwrap_or(m.none),
            ),
            pair(m.ack, link.last_ack_at.as_deref().unwrap_or(m.none)),
            pair(
                m.rtt,
                link.rtt_ms
                    .map(|value| format!("{value} ms"))
                    .unwrap_or_else(|| m.unknown.into()),
            ),
            pair(m.outbox, link.outbox_depth),
            pair(m.epoch, link.key_epoch),
            pair(m.replaced, link.replaced_count),
            pair(m.grant_revision, status.grant_revision),
            pair(m.grant_stale, yes(status.grant_stale, m)),
        ]);
        if let Some(error) = &status.inventory_error {
            lines.push(warn(error));
        }
        for check in crate::world::doctor::link_checks(status) {
            if check.status == crate::control::doctor::DoctorCheckStatus::Fail {
                lines.push(warn(check.detail));
                if let Some(remedy) = check.remedy {
                    lines.push(plain(remedy));
                }
            }
        }
    }
    lines
}

fn detail(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    match app.page {
        Page::Machines => app
            .selected_node()
            .map(|node| node_details(node, m))
            .unwrap_or_else(|| vec![plain(m.empty)]),
        Page::Grants => app
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.node.get())
            .and_then(|node| node.grants.grants.get(app.selected[Page::Grants.index()]))
            .map(|entry| {
                vec![
                    pair("ID", &entry.id),
                    pair(m.principal, &entry.grant.principal),
                    pair(m.targets, entry.grant.nodes.join(", ")),
                    pair(m.verbs, entry.grant.verbs.join(", ")),
                    pair(m.expires, entry.grant.expires.as_deref().unwrap_or(m.none)),
                    plain(""),
                    plain(m.grants_help),
                    plain(format!("a  →  {}", m.delete_grant)),
                ]
            })
            .unwrap_or_else(|| vec![plain(m.empty)]),
        Page::Events => app
            .events
            .get(app.selected[Page::Events.index()])
            .map(|event| {
                vec![
                    pair(m.cursor, event.seq),
                    pair(m.observed, &event.at),
                    pair(m.event_kind, &event.kind),
                    pair(m.subject, &event.subject),
                    plain(
                        serde_json::to_string_pretty(&event.facts)
                            .unwrap_or_else(|error| error.to_string()),
                    ),
                ]
            })
            .unwrap_or_else(|| vec![plain(m.empty)]),
        Page::Hub => app
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.hub.get())
            .and_then(|hub| hub.members.get(app.selected[Page::Hub.index()]))
            .map(|member| {
                vec![
                    pair(m.name, &member.name),
                    pair(m.health, state_name(&member.state, m)),
                    pair(m.last_seen, &member.last_seen),
                    pair(m.outbox, member.outbox),
                    pair(m.version, &member.version),
                    plain(format!("a  →  {}", m.revoke)),
                ]
            })
            .unwrap_or_else(|| vec![plain(m.empty)]),
        _ => overview(app, m),
    }
}

fn render_hub(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let mut intro = vec![
        pair(m.directory, app.hub_data.to_string_lossy()),
        plain(m.hub_help),
    ];
    match app.snapshot.as_ref().map(|snapshot| &snapshot.hub) {
        Some(ReadState::Ready(hub)) => {
            if !crate::world::hub::status_is_live(hub) {
                intro.push(warn(m.stale));
            }
            intro.extend([
                pair(m.listen, &hub.listen),
                pair(m.tls, &hub.tls),
                pair(m.title, &hub.world_id),
                pair(m.pending_invites, hub.open_invites),
                pair(m.data_connections, hub.data_connections),
                pair(m.rotation_pending, yes(hub.rotation_pending, m)),
                pair(m.observed, &hub.written_at),
            ]);
            let items = hub
                .members
                .iter()
                .map(|member| {
                    format!(
                        "{}  {}  {} {}",
                        member.name,
                        state_name(&member.state, m),
                        m.outbox,
                        member.outbox
                    )
                })
                .collect();
            intro_list(
                frame,
                area,
                m.hub,
                intro,
                items,
                app.selected[Page::Hub.index()],
            );
        }
        Some(ReadState::Failed(error)) => {
            intro.extend(failure_lines(error, m));
            scroll(frame, app, area, m.hub, intro);
        }
        Some(ReadState::Missing) => {
            intro.push(plain(m.hub_absent));
            intro.push(plain(format!("a  →  {}", m.choose_hub)));
            scroll(frame, app, area, m.hub, intro);
        }
        None => {
            intro.push(plain(m.loading));
            scroll(frame, app, area, m.hub, intro);
        }
    }
}

fn render_form(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let Some(form) = &app.form else {
        return;
    };
    let inner = card(frame, area, form.title(m));
    if inner.height == 0 {
        return;
    }
    let selected = form.selected;
    let help_height = if inner.height >= 12 {
        4
    } else if inner.height >= 7 {
        2
    } else {
        1
    };
    let parts = Layout::default()
        .constraints([Constraint::Min(1), Constraint::Length(help_height)])
        .split(inner);
    let mut rows = Vec::new();
    let mut positions = Vec::new();
    for (index, field) in form.fields.iter().enumerate() {
        positions.push(rows.len());
        let style = if selected == index {
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::muted())
        };
        if matches!(
            field.kind,
            super::forms::FieldKind::Network | super::forms::FieldKind::AutoService
        ) {
            rows.push(Line::from(Span::styled(
                format!(
                    "{} {}",
                    if selected == index { "›" } else { " " },
                    field.kind.label(m, &form.kind)
                ),
                style,
            )));
            let value = if matches!(field.kind, super::forms::FieldKind::AutoService) {
                if field.input.text() == "yes" {
                    m.yes
                } else {
                    m.no
                }
            } else {
                field.input.text()
            };
            rows.push(Line::from(Span::styled(
                format!("  ← [ {value} ] →"),
                choice_style(selected == index),
            )));
            continue;
        }
        rows.push(Line::from(Span::styled(
            format!(
                "{} {}  {}/{}",
                if selected == index { "›" } else { " " },
                field.kind.label(m, &form.kind),
                field.input.len(),
                field.input.limit()
            ),
            style,
        )));
        let cells = field.input.cells(parts[0].width.saturating_sub(2) as usize);
        let mut spans = vec![Span::raw("  ")];
        for cell in cells {
            let mut style = Style::default().fg(theme::fg());
            if cell.selected || (cell.cursor && selected == index) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            spans.push(Span::styled(cell.text, style));
        }
        rows.push(Line::from(spans));
        if let Some(error) = &field.input.error {
            rows.push(warn(match error {
                InputError::TooLong { limit, count } => m
                    .too_long
                    .replace("{limit}", &limit.to_string())
                    .replace("{count}", &count.to_string()),
                InputError::SingleLine => m.single_line.into(),
            }));
        }
    }
    positions.push(rows.len());
    rows.push(Line::from(Span::styled(
        format!(
            "{} [ {} ]",
            if selected == form.fields.len() {
                "›"
            } else {
                " "
            },
            if matches!(
                form.kind,
                super::forms::FormKind::Description
                    | super::forms::FormKind::PrivateNote(_)
                    | super::forms::FormKind::Export(_)
            ) {
                m.save
            } else {
                m.continue_action
            }
        ),
        Style::default()
            .fg(theme::accent())
            .add_modifier(if selected == form.fields.len() {
                Modifier::REVERSED
            } else {
                Modifier::BOLD
            }),
    )));
    let current = positions[selected.min(positions.len() - 1)];
    let offset = current.saturating_sub(parts[0].height.saturating_sub(2) as usize);
    frame.render_widget(
        Paragraph::new(rows).scroll((offset.min(u16::MAX as usize) as u16, 0)),
        parts[0],
    );
    let help = if let Some(error) = &form.error {
        let detail = if form
            .fields
            .get(selected)
            .is_some_and(|field| field.input.text().is_empty())
        {
            m.required
        } else {
            &error.detail
        };
        vec![warn(format!("{}: {}", m.validation, detail))]
    } else if let Some(field) = form.fields.get(selected) {
        vec![plain(field.kind.help(m, &form.kind))]
    } else {
        vec![plain(m.form_keys)]
    };
    frame.render_widget(
        Paragraph::new(wrapped(help, parts[1].width as usize))
            .style(Style::default().fg(theme::muted())),
        parts[1],
    );
}

fn render_confirmation(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
) {
    let discard = app.screen == Screen::Discard;
    let body = if discard {
        vec![warn(m.discard_body)]
    } else {
        preview(app, m)
    };
    let parts = Layout::default()
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(area);
    scroll(
        frame,
        app,
        parts[0],
        if discard { m.discard } else { m.preview },
        body,
    );
    let no = if discard { m.keep_editing } else { m.cancel };
    let yes = if discard { m.discard } else { m.confirm };
    if area.width < 60 {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!("[ {no} ]"),
                    choice_style(!app.confirm_yes),
                )),
                Line::from(Span::styled(
                    format!("[ {yes} ]"),
                    choice_style(app.confirm_yes),
                )),
            ]),
            parts[1],
        );
        return;
    }
    let choices = Line::from(vec![
        Span::styled(format!("  [ {no} ]  "), choice_style(!app.confirm_yes)),
        Span::styled(format!("[ {yes} ]"), choice_style(app.confirm_yes)),
    ]);
    frame.render_widget(Paragraph::new(choices), parts[1]);
}

fn action_name(action: &Action, m: &i18n::WorldMessages) -> &'static str {
    match action {
        Action::Snapshot { .. } => m.inspect,
        Action::Refresh => m.refresh,
        Action::Reconnect => m.reconnect,
        Action::Events { .. } => m.next_events,
        Action::Init { .. } => m.create,
        Action::Enroll { .. } => m.join,
        Action::Description { .. } => m.edit_description,
        Action::PrivateNote { .. } => m.edit_note,
        Action::Invite { .. } => m.invite,
        Action::Revoke { .. } | Action::HubRevoke { .. } => m.revoke,
        Action::Grant { .. } => m.add_grant,
        Action::DeleteGrant { .. } => m.delete_grant,
        Action::Leave => m.leave,
        Action::ExportInvite { .. } => m.export,
        Action::Service { action } => match action {
            ServiceAction::Install => m.install,
            ServiceAction::Start => m.start,
            ServiceAction::Restart => m.restart,
            ServiceAction::Stop => m.stop,
            ServiceAction::Uninstall => m.uninstall,
        },
    }
}

fn preview(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    let Some(action) = &app.preview else {
        return vec![plain(m.empty)];
    };
    match action {
        Action::Init {
            hub,
            name,
            network,
            install_service,
            ..
        } => vec![
            warn(m.enroll_warning),
            plain(""),
            pair(m.name, name),
            pair(m.hub_address, hub),
            pair(m.network, network.as_str()),
            pair(m.auto_service, yes(*install_service, m)),
        ],
        Action::Enroll {
            invite,
            name,
            network,
            install_service,
            ..
        } => {
            let hub = crate::world::invite::Invite::parse(invite)
                .map(|invite| invite.hub.join(", "))
                .unwrap_or_else(|_| m.unknown.into());
            vec![
                warn(m.enroll_warning),
                plain(""),
                pair(m.name, name),
                pair(m.hub_address, hub),
                pair(m.network, network.as_str()),
                pair(m.auto_service, yes(*install_service, m)),
            ]
        }
        Action::Invite { name, hours, hubs } => vec![
            plain(m.hours_help),
            pair(m.name, name.as_deref().unwrap_or(m.optional_help)),
            pair(m.hours, hours),
            pair(
                m.hub_address,
                if hubs.is_empty() {
                    m.optional_help.into()
                } else {
                    hubs.join(", ")
                },
            ),
        ],
        Action::Revoke { name } => vec![warn(m.revoke_warning), plain(""), pair(m.name, name)],
        Action::Leave => vec![
            warn(m.leave_warning),
            pair(
                m.this_machine,
                app.snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.config.get())
                    .map(|config| config.name.as_str())
                    .unwrap_or(m.unknown),
            ),
        ],
        Action::Grant {
            grant,
            expected_revision,
        } => vec![
            warn(m.grant_warning),
            plain(""),
            pair(m.principal, &grant.principal),
            pair(m.targets, grant.nodes.join(", ")),
            pair(m.verbs, grant.verbs.join(", ")),
            pair(m.expires, grant.expires.as_deref().unwrap_or(m.none)),
            pair(m.grant_revision, expected_revision),
        ],
        Action::DeleteGrant {
            id,
            expected_revision,
        } => {
            let mut lines = vec![
                warn(m.delete_grant_warning),
                pair("ID", id),
                pair(m.grant_revision, expected_revision),
            ];
            if let Some(entry) = app
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.node.get())
                .and_then(|node| node.grants.grants.iter().find(|entry| entry.id == *id))
            {
                lines.extend([
                    pair(m.principal, &entry.grant.principal),
                    pair(m.targets, entry.grant.nodes.join(", ")),
                    pair(m.verbs, entry.grant.verbs.join(", ")),
                ]);
            }
            lines
        }
        Action::Service { action } => vec![
            warn(m.service_warning),
            pair(
                m.service,
                match action {
                    ServiceAction::Install => m.install,
                    ServiceAction::Start => m.start,
                    ServiceAction::Restart => m.restart,
                    ServiceAction::Stop => m.stop,
                    ServiceAction::Uninstall => m.uninstall,
                },
            ),
        ],
        Action::HubRevoke { data, name } => vec![
            warn(m.hub_revoke_warning),
            pair(m.name, name),
            pair(m.directory, data.to_string_lossy()),
        ],
        _ => vec![plain(m.continue_action)],
    }
}

fn render_receipt(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let Some(receipt) = &app.receipt else {
        return;
    };
    let mut lines = Vec::new();
    if receipt.enrolled {
        lines.push(plain(m.enrollment_done));
    }
    if receipt.left {
        lines.push(plain(m.leave_done));
    }
    for (key, value) in &receipt.facts {
        let label = match key.as_str() {
            "member" => m.name,
            "world" => m.title,
            "hub" => m.hub_address,
            "tls" => m.tls,
            "epoch" => m.epoch,
            "service" => m.service,
            "description" => m.description,
            "private_note" => m.private_note,
            "revision" => m.grant_revision,
            "grant" => m.grants,
            "file" => m.path,
            "hours" => m.hours,
            _ => m.facts,
        };
        let value = match value.as_str() {
            "saved" => m.saved,
            "queued" => m.queued,
            "waiting_for_node" => m.waiting_node,
            "waiting_for_connection" => m.waiting_connection,
            "not_installed" => m.not_installed,
            _ => value,
        };
        lines.push(pair(label, value));
    }
    for warning in &receipt.warnings {
        lines.push(warn(warning));
    }
    if let Some(invite) = &receipt.invite {
        lines.extend([
            plain(""),
            plain(m.invite_help),
            plain(""),
            plain(invite),
            plain(""),
            plain(format!("e  {}", m.export)),
        ]);
    }
    scroll(
        frame,
        app,
        area,
        if receipt.warnings.is_empty() {
            m.done
        } else {
            m.partial
        },
        lines,
    );
}

fn help_lines(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    if app.help_from == Screen::Form
        && let Some(form) = &app.form
    {
        if let Some(field) = form.fields.get(form.selected) {
            return vec![
                plain(field.kind.label(m, &form.kind)),
                plain(""),
                plain(field.kind.help(m, &form.kind)),
                plain(""),
                plain(m.form_keys),
                plain(m.edit_keys),
            ];
        }
        return vec![plain(m.form_keys), plain(m.edit_keys)];
    }
    if app.page != Page::Hub && !app.enrolled() {
        return vec![
            plain(m.welcome),
            plain(""),
            plain(m.welcome_body),
            plain(""),
            plain(m.setup_help),
            plain(""),
            plain(m.join_help),
            plain(""),
            plain(m.shortcuts),
        ];
    }
    let mut lines = match app.page {
        Page::Overview => overview(app, m),
        Page::Service => {
            let mut lines = overview(app, m);
            lines.push(plain(m.service_warning));
            lines
        }
        Page::Machines => detail(app, m),
        Page::Grants => {
            let mut lines = vec![plain(m.grants_help), plain(m.grants_default)];
            lines.extend(detail(app, m));
            lines
        }
        Page::Health => health(app, m),
        Page::Events => {
            let mut lines = vec![
                plain(m.cursor_help),
                pair(m.cursor, app.event_cursor),
                pair(m.events, app.event_latest),
            ];
            if let Some(error) = &app.event_error {
                lines.extend(failure_lines(error, m));
            }
            lines.extend(detail(app, m));
            lines
        }
        Page::Hub => {
            let mut lines = vec![
                pair(m.directory, app.hub_data.to_string_lossy()),
                plain(m.hub_help),
            ];
            if let Some(hub) = app
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.hub.get())
            {
                lines.extend([
                    pair(m.listen, &hub.listen),
                    pair(m.tls, &hub.tls),
                    pair(m.title, &hub.world_id),
                    pair(m.pending_invites, hub.open_invites),
                    pair(m.data_connections, hub.data_connections),
                    pair(m.rotation_pending, yes(hub.rotation_pending, m)),
                    pair(m.observed, &hub.written_at),
                ]);
            }
            lines
        }
    };
    lines.push(plain(""));
    lines.push(plain(m.actions));
    for action in app.actions() {
        lines.push(plain(format!("• {}", action.label(m))));
    }
    lines
}

fn failure_lines(error: &Failure, m: &i18n::WorldMessages) -> Lines {
    let title = match error.kind {
        FailureKind::Permission => m.permission,
        FailureKind::Unavailable => m.unavailable,
        FailureKind::Invalid => m.validation,
        _ => m.error,
    };
    let mut lines = vec![warn(title), plain(&error.detail)];
    if error.uncertain {
        lines.push(warn(m.uncertain));
    }
    lines
}
fn state_name<'a>(state: &'a str, m: &'a i18n::WorldMessages) -> &'a str {
    match state {
        "online" => m.online,
        "offline" => m.offline,
        "revoked" => m.revoked,
        _ => state,
    }
}
fn link_name(state: &LinkState, m: &i18n::WorldMessages) -> &'static str {
    match state {
        LinkState::Connected => m.connected,
        LinkState::Starting | LinkState::Connecting { .. } | LinkState::Reconnecting { .. } => {
            m.connecting
        }
        LinkState::Stopped { .. } => m.stopped,
    }
}
fn yes(value: bool, m: &i18n::WorldMessages) -> &'static str {
    if value { m.yes } else { m.no }
}
