//! Adaptive World views: a picture of the World first, then facts grouped by the question each
//! page answers, with the actions the current state allows.

use super::model::{
    self, Presence, Tone, activity_line, current_status, os_label, relative, verdict,
};
use super::raster::{Dot, Stroke, Surface};
use super::topology::{self, Hub, Machine, Topology};
use super::widgets::{
    Lines, bar, card, choice_style, clipped, intro_list, judged, list, muted, pair, plain, safe,
    scroll, section, warn, wrapped,
};
use super::{MenuAction, Page, Screen, WorldApp, i18n};
use crate::control::i18n::Language;
use crate::tui::text_input::InputError;
use crate::tui::theme;
use crate::world::client::NodeView;
use crate::world::control::{Action, Failure, FailureKind, ReadState, ServiceAction};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Tabs};

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut WorldApp, language: Language, area: Rect) {
    let m = i18n::messages(language);
    let show_tabs =
        matches!(app.screen, Screen::Page | Screen::Detail | Screen::Actions) && area.height >= 16;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(u16::from(show_tabs)),
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
    if show_tabs {
        render_tabs(frame, app, m, rows[1]);
    }
    match app.screen {
        Screen::Page => render_page(frame, app, m, rows[2]),
        Screen::Detail => {
            let lines = detail(app, m);
            scroll(frame, app, rows[2], app.page.title(m), lines);
        }
        Screen::Actions => {
            let recommended = app.recommended();
            let items = app
                .actions()
                .iter()
                .map(|action| action_item(*action, recommended, m))
                .collect::<Vec<_>>();
            list(frame, rows[2], m.actions, items, app.menu_selected);
        }
        Screen::Form => render_form(frame, app, m, rows[2]),
        Screen::Confirm | Screen::Discard => render_confirmation(frame, app, m, rows[2]),
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
            scroll(frame, app, rows[2], m.busy, lines);
        }
        Screen::Failed => {
            let lines = app
                .failure
                .as_ref()
                .map(|failure| failure_lines(failure, m))
                .unwrap_or_else(|| vec![plain(m.failed)]);
            scroll(frame, app, rows[2], m.failed, lines);
        }
        Screen::Receipt => render_receipt(frame, app, m, rows[2]),
        Screen::Help => {
            let lines = help_lines(app, m);
            scroll(frame, app, rows[2], m.facts, lines);
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
                    "← → 1-7  {}/7   a {}  F1",
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
                    "{}  ·  {}/7  ·  r {}",
                    m.page_keys,
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
        rows[3],
    );
}

fn render_tabs(frame: &mut Frame<'_>, app: &WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let titles = Page::ALL
        .iter()
        .enumerate()
        .map(|(index, page)| format!("{} {}", index + 1, page.title(m)))
        .collect::<Vec<_>>();
    let needed = titles
        .iter()
        .map(|title| Span::raw(title.as_str()).width() + 3)
        .sum::<usize>()
        + 1;
    if needed <= usize::from(area.width) {
        frame.render_widget(
            Tabs::new(titles)
                .select(app.page.index())
                .style(Style::default().fg(theme::muted()))
                .highlight_style(
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                )
                .divider(" "),
            area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("‹ ", Style::default().fg(theme::muted())),
                Span::styled(
                    format!("{}/7  {}", app.page.index() + 1, app.page.title(m)),
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ›", Style::default().fg(theme::muted())),
            ])),
            area,
        );
    }
}

fn action_item(
    action: MenuAction,
    recommended: Option<MenuAction>,
    m: &i18n::WorldMessages,
) -> Line<'static> {
    let mut spans = vec![Span::raw(action.label(m).to_string())];
    if let Some(letter) = action.hotkey() {
        spans.push(Span::styled(
            format!("  {letter}"),
            Style::default().fg(theme::muted()),
        ));
    }
    if recommended == Some(action) {
        spans.push(Span::styled(
            format!("  ★ {}", m.recommended),
            Style::default().fg(theme::accent()),
        ));
    }
    Line::from(spans)
}

/// The row of lettered shortcuts under a page: `i Invite a machine   s Node service …`.
fn hotkey_row(actions: &[MenuAction], m: &i18n::WorldMessages, width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut used = 0;
    let mut push = |letter: char, label: &str, spans: &mut Vec<Span<'static>>| {
        let text = format!(" {label}   ");
        let size = 1 + Span::raw(text.as_str()).width();
        if used + size > width {
            return false;
        }
        used += size;
        spans.push(Span::styled(
            letter.to_string(),
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(text, Style::default().fg(theme::muted())));
        true
    };
    for action in actions {
        if let Some(letter) = action.hotkey()
            && !push(letter, action.label(m), &mut spans)
        {
            break;
        }
    }
    push('a', m.actions, &mut spans);
    Line::from(spans)
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
    let welcome = matches!(snapshot.config, ReadState::Missing);
    if let ReadState::Failed(failure) = &snapshot.config {
        let lines = failure_lines(failure, m);
        scroll(frame, app, area, m.error, lines);
        return;
    }
    if welcome {
        render_welcome(frame, app, m, area);
        return;
    }
    match app.page {
        Page::Overview => render_overview(frame, app, m, area),
        Page::Machines => render_machines(frame, app, m, area),
        Page::Grants => render_grants(frame, app, m, area),
        Page::Connection => render_connection(frame, app, m, area),
        Page::Events => render_events(frame, app, m, area),
        Page::Service => render_service(frame, app, m, area),
        Page::Hub => unreachable!(),
    }
}

fn render_welcome(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let sketch_rows = if area.height >= 18 && area.width >= 56 {
        9
    } else {
        0
    };
    let parts = Layout::default()
        .constraints([Constraint::Length(sketch_rows), Constraint::Min(5)])
        .split(area);
    if sketch_rows > 0 {
        let inner = card(frame, parts[0], m.title);
        topology::sketch(frame.buffer_mut(), inner, m);
    }
    let items = app
        .actions()
        .iter()
        .map(|action| plain(action.label(m)))
        .collect();
    intro_list(
        frame,
        parts[1],
        m.welcome,
        vec![plain(m.welcome_body)],
        items,
        app.selected[app.page.index()],
    );
}

/// The picture the Overview draws, from the live member list or, when the node is down, from
/// the enrollment alone so the user still sees where this machine stands.
fn build_topology(app: &WorldApp, m: &i18n::WorldMessages) -> Topology {
    let snapshot = app.snapshot.as_ref();
    let config = snapshot.and_then(|snapshot| snapshot.config.get());
    let status = snapshot.and_then(current_status);
    let hub = Hub {
        address: config
            .map(|config| config.hub.join(", "))
            .unwrap_or_else(|| m.unknown.to_string()),
        link: status
            .filter(|(_, fresh)| *fresh)
            .map(|(status, _)| model::LinkTone::from_state(&status.link.state)),
    };
    let mut machines = app
        .nodes()
        .iter()
        .map(|node| Machine {
            name: node.name.clone(),
            presence: Presence::from_state(&node.state),
            system: format!("{} · {}", os_label(&node.os), node.arch),
            is_self: node.is_self,
            rtt_ms: node.hub_rtt_ms,
            last_seen: relative(&node.last_seen, m),
            activity: activity_line(node, m),
        })
        .collect::<Vec<_>>();
    if machines.is_empty()
        && let Some(config) = config
    {
        machines.push(Machine {
            name: config.name.clone(),
            presence: Presence::Unknown,
            system: m.this_machine.to_string(),
            is_self: true,
            rtt_ms: None,
            last_seen: status
                .map(|(status, _)| relative(&status.written_at, m))
                .unwrap_or_else(|| m.unknown.to_string()),
            activity: None,
        });
    }
    Topology {
        hub,
        machines,
        selected: (!app.nodes().is_empty()).then_some(app.selected[Page::Overview.index()]),
    }
}

fn render_overview(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let Some(snapshot) = &app.snapshot else {
        return;
    };
    let verdict = verdict(snapshot, m);
    let world_id = snapshot
        .config
        .get()
        .map(|config| config.world_id.clone())
        .unwrap_or_default();
    let load_error = app.load_error.as_ref().map(|error| error.detail.clone());
    let topology = build_topology(app, m);
    let tall = area.height >= 14;
    let band_rows = if tall || verdict.hint.is_some() || load_error.is_some() {
        2
    } else {
        1
    };
    let parts = Layout::default()
        .constraints([
            Constraint::Min(4),
            Constraint::Length(band_rows),
            Constraint::Length(u16::from(tall)),
        ])
        .split(area);
    let inner = card(frame, parts[0], &format!("{} · {world_id}", m.title));
    topology::render(frame.buffer_mut(), inner, &topology, m);
    let mut band = vec![judged(
        verdict.tone.symbol(),
        verdict.tone.color(),
        &verdict.text,
    )];
    if let Some(error) = load_error {
        band.push(warn(error));
    } else if let Some(hint) = verdict.hint {
        band.push(Line::from(Span::styled(
            format!("  → {hint}"),
            Style::default().fg(theme::accent()),
        )));
    } else {
        band.push(muted(format!(
            "  {}  ● {}  ○ {}  ✕ {}  ★ {}",
            m.go_machine, m.online, m.offline, m.revoked, m.this_machine
        )));
    }
    frame.render_widget(
        Paragraph::new(wrapped(band, parts[1].width as usize)),
        parts[1],
    );
    if tall {
        frame.render_widget(
            Paragraph::new(hotkey_row(&app.actions(), m, parts[2].width as usize)),
            parts[2],
        );
    }
}

fn render_machines(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    if let Some(lines) = node_error(app, m, true) {
        scroll(frame, app, area, m.machines, lines);
        return;
    }
    let nodes = app.nodes();
    if nodes.is_empty() {
        scroll(frame, app, area, m.machines, vec![plain(m.empty)]);
        return;
    }
    let wide = area.width >= 100 && area.height >= 18;
    let panes = if wide {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
            .split(area)
    } else {
        Layout::default()
            .constraints([Constraint::Percentage(100)])
            .split(area)
    };
    let hint_rows = u16::from(nodes.len() == 1 && area.height >= 12);
    let parts = Layout::default()
        .constraints([Constraint::Min(4), Constraint::Length(hint_rows)])
        .split(panes[0]);
    let inner = card(frame, parts[0], m.machines);
    let compact = inner.width < 64;
    let header = if compact {
        vec![m.state, m.name, m.system, m.latency]
    } else {
        vec![m.state, m.name, m.system, m.latency, m.activity, m.note]
    };
    let mut widths = header
        .iter()
        .map(|title| Span::raw(*title).width() as u16)
        .collect::<Vec<_>>();
    let rows = nodes
        .iter()
        .map(|node| {
            let presence = Presence::from_state(&node.state);
            let latency = match (presence, node.hub_rtt_ms) {
                (Presence::Online, Some(rtt)) => format!("{rtt} ms"),
                (Presence::Online, None) => String::new(),
                _ => relative(&node.last_seen, m),
            };
            let mut cells = vec![
                (
                    format!("{} {}", presence.symbol(), presence.label(m)),
                    Style::default().fg(presence.color()),
                ),
                (
                    format!(
                        "{}{}",
                        if node.is_self { "★ " } else { "" },
                        safe(&node.name)
                    ),
                    Style::default()
                        .fg(if node.is_self {
                            theme::accent()
                        } else {
                            theme::fg()
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                (
                    format!("{} · {}", os_label(&node.os), node.arch),
                    Style::default().fg(theme::muted()),
                ),
                (latency, Style::default().fg(theme::fg())),
            ];
            if !compact {
                cells.push((
                    activity_line(node, m).unwrap_or_else(|| "—".to_string()),
                    Style::default().fg(theme::muted()),
                ));
                cells.push((
                    node.effective_note().map(safe).unwrap_or_default(),
                    Style::default().fg(theme::fg()),
                ));
            }
            for (index, (text, _)) in cells.iter().enumerate() {
                widths[index] = widths[index].max(Span::raw(text.as_str()).width() as u16);
            }
            Row::new(
                cells
                    .into_iter()
                    .map(|(text, style)| Cell::from(Span::styled(text, style)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    let constraints = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            if index + 1 == widths.len() {
                Constraint::Min((*width).min(8))
            } else {
                Constraint::Length((*width).min(28))
            }
        })
        .collect::<Vec<_>>();
    let mut state = TableState::default().with_selected(Some(app.selected[Page::Machines.index()]));
    frame.render_stateful_widget(
        Table::new(rows, constraints)
            .header(
                Row::new(
                    header
                        .iter()
                        .map(|title| {
                            Cell::from(Span::styled(*title, Style::default().fg(theme::muted())))
                        })
                        .collect::<Vec<_>>(),
                )
                .bottom_margin(0),
            )
            .column_spacing(2)
            .highlight_symbol("› ")
            .row_highlight_style(
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::REVERSED),
            ),
        inner,
        &mut state,
    );
    if hint_rows > 0 {
        frame.render_widget(
            Paragraph::new(wrapped(vec![muted(m.only_self)], parts[1].width as usize)),
            parts[1],
        );
    }
    if wide {
        let lines = app
            .focused_node()
            .map(|node| node_details(node, m))
            .unwrap_or_default();
        scroll(frame, app, panes[1], m.facts, lines);
    }
}

fn render_grants(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
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
            plain(""),
            judged(
                Tone::Info.symbol(),
                Tone::Info.color(),
                if node.grants.revision == 0 {
                    m.grants_default
                } else {
                    m.grants_empty
                },
            ),
            plain(""),
            muted(m.grants_help),
            plain(""),
            plain(format!("e  →  {}", m.add_grant)),
        ];
        scroll(frame, app, area, m.grants, lines);
        return;
    }
    let intro = wrapped(
        vec![
            pair(m.grant_revision, node.grants.revision),
            muted(m.grants_help),
        ],
        area.width.saturating_sub(2) as usize,
    );
    let intro_height = (intro.len() as u16).min(area.height.saturating_sub(6));
    let parts = Layout::default()
        .constraints([Constraint::Length(intro_height), Constraint::Min(4)])
        .split(area);
    frame.render_widget(Paragraph::new(intro), parts[0]);
    let inner = card(frame, parts[1], m.grants);
    let header = [m.principal, m.targets, m.verbs, m.expires];
    let mut widths = header
        .iter()
        .map(|title| Span::raw(*title).width() as u16)
        .collect::<Vec<_>>();
    let rows = node
        .grants
        .grants
        .iter()
        .map(|entry| {
            let cells = [
                safe(&entry.grant.principal),
                entry.grant.nodes.join(", "),
                entry.grant.verbs.join(", "),
                entry
                    .grant
                    .expires
                    .clone()
                    .unwrap_or_else(|| m.none.to_string()),
            ];
            for (index, text) in cells.iter().enumerate() {
                widths[index] = widths[index].max(Span::raw(text.as_str()).width() as u16);
            }
            Row::new(cells.into_iter().map(Cell::from).collect::<Vec<_>>())
        })
        .collect::<Vec<_>>();
    let constraints = widths
        .iter()
        .map(|width| Constraint::Length((*width).min(30)))
        .collect::<Vec<_>>();
    let mut state = TableState::default().with_selected(Some(app.selected[Page::Grants.index()]));
    frame.render_stateful_widget(
        Table::new(rows, constraints)
            .header(Row::new(
                header
                    .iter()
                    .map(|title| {
                        Cell::from(Span::styled(*title, Style::default().fg(theme::muted())))
                    })
                    .collect::<Vec<_>>(),
            ))
            .column_spacing(2)
            .highlight_symbol("› ")
            .row_highlight_style(
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::REVERSED),
            ),
        inner,
        &mut state,
    );
}

fn connection_lines(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    let Some(snapshot) = &app.snapshot else {
        return vec![plain(m.loading)];
    };
    let mut lines = Vec::new();
    if let Some(error_lines) = node_error(app, m, false) {
        lines.extend(error_lines);
    }
    let Some((status, fresh)) = current_status(snapshot) else {
        return lines;
    };
    if !fresh {
        lines.push(warn(m.stale));
        lines.push(pair(m.data_time, relative(&status.written_at, m)));
    }
    lines.push(section(m.checks));
    for finding in crate::world::doctor::link_findings(status) {
        let view = model::finding_view(&finding, m);
        lines.push(judged(view.tone.symbol(), view.tone.color(), view.text));
        if let Some(remedy) = view.remedy {
            lines.push(muted(format!("    → {remedy}")));
        }
    }
    if let Some(node) = snapshot.node.get() {
        for (name, reason) in &node.unverified {
            lines.push(judged(
                Tone::Warn.symbol(),
                Tone::Warn.color(),
                format!("{name}: {reason}"),
            ));
        }
    }
    let link = &status.link;
    lines.extend([
        plain(""),
        section(m.hub_address),
        pair(m.hub_address, status.hub.join(", ")),
        pair(m.network, link.path.as_deref().unwrap_or(m.unknown)),
        pair(m.interface, link.interface.as_deref().unwrap_or(m.none)),
        pair(
            "TUN",
            if link.tunnels.is_empty() {
                m.none.to_string()
            } else {
                link.tunnels.join(", ")
            },
        ),
        pair(m.tls, link.tls.as_str()),
        plain(""),
        section(m.heartbeat),
        pair(
            m.heartbeat,
            link.last_heartbeat_at
                .as_deref()
                .map(|at| relative(at, m))
                .unwrap_or_else(|| m.none.to_string()),
        ),
        pair(
            m.ack,
            link.last_ack_at
                .as_deref()
                .map(|at| relative(at, m))
                .unwrap_or_else(|| m.none.to_string()),
        ),
        pair(
            m.rtt,
            link.rtt_ms
                .map(|value| format!("{value} ms"))
                .unwrap_or_else(|| m.unknown.into()),
        ),
        pair(m.outbox, link.outbox_depth),
        plain(""),
        section(m.grants),
        pair(m.epoch, link.key_epoch),
        pair(m.replaced, link.replaced_count),
        pair(m.grant_revision, status.grant_revision),
        pair(m.grant_stale, yes(status.grant_stale, m)),
        pair(m.remote_ready, yes(status.engine_hosted, m)),
        pair(m.running_calls, status.running_calls),
    ]);
    if let Some(error) = &status.inventory_error {
        lines.push(warn(error));
    }
    lines
}

fn render_connection(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
) {
    let lines = connection_lines(app, m);
    let chart_rows = if area.height >= 18 && app.rtt_history.len() >= 2 {
        7
    } else {
        0
    };
    let parts = Layout::default()
        .constraints([Constraint::Min(6), Constraint::Length(chart_rows)])
        .split(area);
    scroll(frame, app, parts[0], m.connection, lines);
    if chart_rows > 0 {
        let inner = card(frame, parts[1], m.rtt_history);
        render_rtt_chart(frame, inner, app);
    }
}

/// A line graph of recent round trips at dot resolution: one sample per column, latest on the
/// right, the scale printed at both ends.
fn render_rtt_chart(frame: &mut Frame<'_>, area: Rect, app: &WorldApp) {
    if area.width < 12 || area.height < 2 {
        return;
    }
    let label_width = 8u16;
    let plot_columns = area.width - label_width;
    let samples = app
        .rtt_history
        .iter()
        .rev()
        .take(usize::from(plot_columns))
        .rev()
        .copied()
        .collect::<Vec<_>>();
    let Some(&max) = samples.iter().max() else {
        return;
    };
    let top = max.max(1);
    let mut surface = Surface::new(area.width, area.height, super::raster::glyphs());
    let height = f64::from(area.height) * 4.0 - 1.0;
    let origin_x = f64::from(label_width) * 2.0;
    let y_of = |value: u32| height - (f64::from(value) / f64::from(top)) * height;
    surface.line(
        Dot::new(origin_x, height),
        Dot::new(f64::from(area.width) * 2.0 - 1.0, height),
        theme::border(),
        Stroke::Dashed,
    );
    let mut previous: Option<Dot> = None;
    let start = usize::from(plot_columns) - samples.len();
    for (index, value) in samples.iter().enumerate() {
        let point = Dot::new(origin_x + ((start + index) * 2) as f64, y_of(*value));
        if let Some(previous) = previous {
            surface.line(previous, point, theme::success(), Stroke::Solid);
        } else {
            surface.set(point.x as i32, point.y as i32, theme::success());
        }
        previous = Some(point);
    }
    let style = Style::default().fg(theme::muted());
    surface.label(0, 0, format!("{top} ms"), style, i32::from(label_width) - 1);
    surface.label(
        0,
        i32::from(area.height) - 1,
        "0 ms",
        style,
        i32::from(label_width) - 1,
    );
    if let Some(last) = samples.last() {
        let text = format!(" {last} ms ");
        let width = Span::raw(text.as_str()).width() as i32;
        let row = (y_of(*last) / 4.0).round() as i32;
        surface.label(
            i32::from(area.width) - width,
            row.clamp(0, i32::from(area.height) - 1),
            text,
            Style::default().fg(theme::accent()).bg(theme::bg()),
            width,
        );
    }
    surface.render(area, frame.buffer_mut());
}

fn render_events(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    if let Some(error) = &app.event_error {
        let mut lines = failure_lines(error, m);
        lines.push(pair(m.cursor, app.event_cursor));
        lines.push(plain(format!("l  →  {}", m.next_events)));
        if app.events.is_empty() {
            scroll(frame, app, area, m.events, lines);
            return;
        }
    }
    let mut intro = vec![
        pair(m.cursor, app.event_cursor),
        muted(if app.event_cursor < app.event_latest {
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
        return;
    }
    let selected = app.selected[Page::Events.index()];
    let start = selected.saturating_sub(area.height as usize);
    let when_width = app
        .events
        .iter()
        .map(|event| Span::raw(relative(&event.at, m).as_str()).width())
        .max()
        .unwrap_or(0);
    let items = app
        .events
        .iter()
        .skip(start)
        .take(area.height.max(1) as usize * 3)
        .map(|event| {
            let (label, tone) = model::event_label(&event.kind, m);
            let when = relative(&event.at, m);
            let padding = when_width.saturating_sub(Span::raw(when.as_str()).width());
            Line::from(vec![
                Span::styled(
                    format!("{}{when}  ", " ".repeat(padding)),
                    Style::default().fg(theme::muted()),
                ),
                Span::styled(
                    format!("{} ", tone.symbol()),
                    Style::default().fg(tone.color()),
                ),
                Span::raw(safe(label)),
                Span::styled(
                    format!("  {}", safe(&event.subject)),
                    Style::default().fg(theme::muted()),
                ),
            ])
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

fn service_lines(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    let Some(snapshot) = &app.snapshot else {
        return vec![plain(m.loading)];
    };
    let mut lines = Vec::new();
    match &snapshot.service_installed {
        ReadState::Failed(error) => lines.push(warn(format!("{}: {}", m.service, error.detail))),
        ReadState::Missing => lines.push(pair(m.service, m.unknown)),
        ReadState::Ready(_) => {}
    }
    if let ReadState::Failed(error) = &snapshot.node {
        lines.push(warn(&error.detail));
    }
    if let Some((status, fresh)) = current_status(snapshot) {
        if !fresh {
            lines.push(warn(m.stale));
        }
        lines.push(pair(m.data_time, relative(&status.written_at, m)));
        lines.push(pair(m.pid, status.pid));
        lines.push(pair(m.version, &status.version));
        lines.push(pair(m.started, relative(&status.started_at, m)));
        lines.push(pair(
            m.ai_sessions,
            status
                .agent_connections
                .map_or_else(|| m.unknown.to_string(), |count| count.to_string()),
        ));
        lines.push(pair(m.running_calls, status.running_calls));
    } else {
        lines.push(judged(
            Tone::Fail.symbol(),
            Tone::Fail.color(),
            m.unavailable,
        ));
        lines.push(muted(m.unavailable_body));
    }
    lines.push(pair(m.log_file, crate::paths::display_path(&app.node_log)));
    lines
}

fn render_service(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let Some(snapshot) = &app.snapshot else {
        return;
    };
    let stages = model::stages(snapshot, m);
    let recommended = app.recommended();
    let stepper_rows = if area.height >= 14 { 4 } else { 0 };
    let parts = Layout::default()
        .constraints([Constraint::Length(stepper_rows), Constraint::Min(4)])
        .split(area);
    if stepper_rows > 0 {
        let inner = card(frame, parts[0], m.this_machine);
        render_stepper(frame, inner, &stages, m);
    }
    let mut intro = Vec::new();
    if stepper_rows == 0 {
        for view in &stages {
            intro.push(judged(
                view.tone.symbol(),
                view.tone.color(),
                format!("{} · {}", view.stage.label(m), view.detail),
            ));
        }
    }
    match recommended {
        Some(action) => intro.push(Line::from(vec![
            Span::styled("→ ", Style::default().fg(theme::accent())),
            Span::styled(
                format!("{}: {}", m.recommended, action.label(m)),
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        None => intro.push(judged(Tone::Ok.symbol(), Tone::Ok.color(), m.service_ok)),
    }
    intro.extend(service_lines(app, m));
    intro.push(muted(m.service_warning));
    let items = app
        .actions()
        .iter()
        .map(|action| action_item(*action, recommended, m))
        .collect();
    intro_list(
        frame,
        parts[1],
        m.actions,
        intro,
        items,
        app.selected[Page::Service.index()],
    );
}

/// Five pills joined by a rule; a pill's fill is its judgement, the row beneath is its fact.
fn render_stepper(
    frame: &mut Frame<'_>,
    area: Rect,
    stages: &[model::StageView],
    m: &i18n::WorldMessages,
) {
    if area.width < 20 || area.height < 2 || stages.is_empty() {
        return;
    }
    let slot = i32::from(area.width) / stages.len() as i32;
    let monochrome = theme::monochrome();
    let mut surface = Surface::new(area.width, area.height, super::raster::glyphs());
    let mut previous_end: Option<i32> = None;
    for (index, view) in stages.iter().enumerate() {
        let label = format!("{} {}", view.tone.symbol(), view.stage.label(m));
        let label_width = Span::raw(label.as_str()).width() as i32;
        let pill = (label_width + 2).min(slot - 1).max(4);
        let left = index as i32 * slot + (slot - pill) / 2;
        let fill = view.tone.color();
        surface.panel(
            left * 2,
            0,
            pill * 2,
            4,
            (!monochrome).then_some(fill),
            monochrome.then_some(fill),
        );
        if let Some(end) = previous_end {
            surface.line(
                Dot::new(f64::from(end * 2), 2.0),
                Dot::new(f64::from(left * 2 - 1), 2.0),
                theme::border(),
                Stroke::Solid,
            );
        }
        previous_end = Some(left + pill);
        let text = if monochrome || view.tone == Tone::Info {
            theme::fg()
        } else {
            theme::bg()
        };
        surface.label(
            left + 1,
            0,
            clipped(&label, (pill - 2) as usize),
            Style::default().fg(text).add_modifier(Modifier::BOLD),
            pill - 2,
        );
        let detail = clipped(&safe(&view.detail), (slot - 1) as usize);
        let detail_width = Span::raw(detail.as_str()).width() as i32;
        surface.label(
            index as i32 * slot + (slot - detail_width) / 2,
            1,
            detail,
            Style::default().fg(theme::muted()),
            slot - 1,
        );
    }
    if area.height >= 3 {
        surface.label(
            0,
            i32::from(area.height) - 1,
            clipped(m.stage_help, usize::from(area.width)),
            Style::default().fg(theme::muted()),
            i32::from(area.width),
        );
    }
    surface.render(area, frame.buffer_mut());
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
        ReadState::Missing => Some(vec![
            judged(Tone::Fail.symbol(), Tone::Fail.color(), m.unavailable),
            plain(m.unavailable_body),
            plain(format!("s  →  {}", m.service)),
        ]),
    }
}

fn node_details(node: &NodeView, m: &i18n::WorldMessages) -> Lines {
    let presence = Presence::from_state(&node.state);
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "{}{}  ·  {} · {}",
                if node.is_self { "★ " } else { "" },
                safe(&node.name),
                os_label(&node.os),
                node.arch
            ),
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                format!("{} {}", presence.symbol(), presence.label(m)),
                Style::default().fg(presence.color()),
            ),
            Span::styled(
                format!(
                    "  ·  {}: {}{}",
                    m.last_seen,
                    relative(&node.last_seen, m),
                    node.hub_rtt_ms
                        .filter(|_| presence == Presence::Online)
                        .map(|rtt| format!("  ·  {rtt} ms"))
                        .unwrap_or_default()
                ),
                Style::default().fg(theme::muted()),
            ),
        ]),
        plain(""),
        section(m.activity),
    ];
    match node
        .inventory
        .as_ref()
        .and_then(|inventory| inventory.activity.as_ref())
    {
        Some(activity) if presence == Presence::Online && activity.is_fresh() => {
            lines.extend([
                pair(m.ai_sessions, activity.agent_connections),
                pair(
                    m.running_calls,
                    activity.local_requests + activity.remote_calls,
                ),
                pair(m.remote_ready, yes(activity.engine_hosted, m)),
                pair(m.data_time, relative(&activity.observed_at, m)),
            ]);
        }
        Some(activity) => {
            lines.push(warn(m.stale));
            lines.push(pair(m.ai_sessions, m.unknown));
            lines.push(pair(m.data_time, relative(&activity.observed_at, m)));
        }
        None => lines.push(pair(m.ai_sessions, m.unknown)),
    }
    lines.extend([
        plain(""),
        section(m.note),
        pair(m.effective_note, node.effective_note().unwrap_or(m.none)),
        pair(
            m.private_note,
            node.private_note.as_deref().unwrap_or(m.none),
        ),
        muted(m.note_help),
        pair(
            m.description,
            node.inventory
                .as_ref()
                .map(|inventory| inventory.description.as_str())
                .filter(|text| !text.is_empty())
                .unwrap_or(m.none),
        ),
        muted(m.description_help),
        plain(""),
        section(m.hardware),
    ]);
    if let Some(inv) = &node.inventory {
        lines.extend([
            pair(m.host, &inv.hostname),
            pair(m.user, &inv.user),
            pair(m.home, &inv.home),
            pair("CPU", inv.cpus),
            pair(m.memory, format!("{} GiB", inv.memory_gb)),
        ]);
        for disk in &inv.disks {
            let used = if disk.total_gb > 0.0 {
                1.0 - disk.free_gb / disk.total_gb
            } else {
                0.0
            };
            let free_share = 1.0 - used;
            let color = if free_share < 0.05 {
                theme::danger()
            } else if free_share < 0.2 {
                theme::warning()
            } else {
                theme::success()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}: ", m.disks),
                    Style::default().fg(theme::muted()),
                ),
                Span::raw(format!("{}  ", safe(&disk.mount))),
                bar(used, 12, color),
                Span::raw(format!("  {} / {} GiB", disk.free_gb, disk.total_gb)),
            ]));
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
        lines.extend([
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
            pair(m.tags, node.tags.join(", ")),
            pair(m.addresses, inv.addresses.join(", ")),
            pair(m.version, &node.version),
            pair(m.inventory_time, relative(&inv.collected_at, m)),
        ]);
        if let Some(error) = &inv.shell.error {
            lines.push(warn(error));
        }
    } else {
        lines.push(pair(m.hardware, m.unknown));
        lines.push(pair(m.tags, node.tags.join(", ")));
        lines.push(pair(m.version, &node.version));
    }
    lines.extend([
        plain(""),
        section(m.connection),
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

fn detail(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    match app.page {
        Page::Overview | Page::Machines => app
            .focused_node()
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
                    muted(m.grants_help),
                    plain(format!("a  →  {}", m.delete_grant)),
                ]
            })
            .unwrap_or_else(|| vec![plain(m.empty)]),
        Page::Events => app
            .events
            .get(app.selected[Page::Events.index()])
            .map(|event| {
                let (label, tone) = model::event_label(&event.kind, m);
                vec![
                    judged(tone.symbol(), tone.color(), label),
                    pair(m.cursor, event.seq),
                    pair(
                        m.data_time,
                        format!("{} · {}", relative(&event.at, m), event.at),
                    ),
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
                let presence = Presence::from_state(&member.state);
                vec![
                    pair(m.name, &member.name),
                    judged(presence.symbol(), presence.color(), presence.label(m)),
                    pair(m.last_seen, relative(&member.last_seen, m)),
                    pair(m.outbox, member.outbox),
                    pair(m.version, &member.version),
                    plain(format!("a  →  {}", m.revoke)),
                ]
            })
            .unwrap_or_else(|| vec![plain(m.empty)]),
        Page::Connection => connection_lines(app, m),
        Page::Service => {
            let mut lines = app
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    model::stages(snapshot, m)
                        .into_iter()
                        .map(|view| {
                            judged(
                                view.tone.symbol(),
                                view.tone.color(),
                                format!("{} · {}", view.stage.label(m), view.detail),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            lines.push(plain(""));
            lines.extend(service_lines(app, m));
            lines
        }
    }
}

fn render_hub(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let mut intro = vec![
        pair(m.directory, app.hub_data.to_string_lossy()),
        muted(m.hub_help),
    ];
    match app.snapshot.as_ref().map(|snapshot| &snapshot.hub) {
        Some(ReadState::Ready(hub)) => {
            let live = crate::world::hub::status_is_live(hub);
            intro.push(if live {
                judged(Tone::Ok.symbol(), Tone::Ok.color(), m.hub_running)
            } else {
                judged(Tone::Warn.symbol(), Tone::Warn.color(), m.hub_not_running)
            });
            intro.extend([
                pair(m.data_time, relative(&hub.written_at, m)),
                pair(m.listen, &hub.listen),
                pair(m.tls, &hub.tls),
                pair(m.title, &hub.world_id),
                pair(m.pending_invites, hub.open_invites),
                pair(m.data_connections, hub.data_connections),
                pair(m.rotation_pending, yes(hub.rotation_pending, m)),
            ]);
            let items = hub
                .members
                .iter()
                .map(|member| {
                    let presence = Presence::from_state(&member.state);
                    Line::from(vec![
                        Span::styled(
                            format!("{} ", presence.symbol()),
                            Style::default().fg(presence.color()),
                        ),
                        Span::raw(format!("{}  ", safe(&member.name))),
                        Span::styled(
                            format!(
                                "{}  ·  {}  ·  {} {}",
                                presence.label(m),
                                relative(&member.last_seen, m),
                                m.outbox,
                                member.outbox
                            ),
                            Style::default().fg(theme::muted()),
                        ),
                    ])
                })
                .collect();
            intro_list(
                frame,
                area,
                m.machines,
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
            intro.push(plain(format!("e  →  {}", m.choose_hub)));
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
        Page::Overview => {
            let mut lines = app
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    let verdict = verdict(snapshot, m);
                    vec![judged(
                        verdict.tone.symbol(),
                        verdict.tone.color(),
                        verdict.text,
                    )]
                })
                .unwrap_or_default();
            lines.push(plain(""));
            lines.extend(detail(app, m));
            lines
        }
        Page::Machines => detail(app, m),
        Page::Grants => {
            let mut lines = vec![plain(m.grants_help), plain(m.grants_default)];
            lines.extend(detail(app, m));
            lines
        }
        Page::Connection => connection_lines(app, m),
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
        Page::Service => {
            let mut lines = vec![plain(m.stage_help), plain("")];
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
                    pair(m.data_time, relative(&hub.written_at, m)),
                ]);
            }
            lines
        }
    };
    lines.push(plain(""));
    lines.push(plain(m.actions));
    let recommended = app.recommended();
    for action in app.actions() {
        lines.push(action_item(action, recommended, m));
    }
    lines.push(plain(""));
    lines.push(plain(m.shortcuts));
    lines.push(plain(m.page_keys));
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
fn yes(value: bool, m: &i18n::WorldMessages) -> &'static str {
    if value { m.yes } else { m.no }
}
