//! The World pages: a picture of the World first, then the facts each page is for, and only
//! the actions the current state allows. Every page keeps one readable column on a small
//! terminal and spends extra width on a detail pane rather than on decoration.

use super::forms::{FieldKind, Focus, FormKind};
use super::model::{
    self, Presence, Tone, activity_line, current_status, os_label, relative, verdict,
};
use super::topology::{self, Hub, Machine, Topology};
use super::widgets::{
    Lines, Tile, bar, button, clipped, dim, elided, judged, muted, notice, padded, pair, plain,
    popup, safe, scroll, section, selection_style, split_width, stat_tiles, strong, table,
    table_window, title_rule, warn, width, wrapped,
};
use super::{MenuAction, Page, Screen, WorldApp, i18n};
use crate::control::i18n::Language;
use crate::tui::text_input::InputError;
use crate::tui::theme;
use crate::world::client::NodeView;
use crate::world::control::{Action, Failure, FailureKind, ReadState, ServiceAction};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row};

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut WorldApp, language: Language, area: Rect) {
    let m = i18n::messages(language);
    app.frame = app.frame.wrapping_add(1);
    app.clamp_selection();
    let regions = super::chrome::render(frame, app, m, area);
    let body = regions.body;
    let compact = regions.compact;
    if app.screen == Screen::Page {
        app.max_scroll = 0;
    }
    match app.screen {
        Screen::Page => render_page(frame, app, m, body, compact),
        Screen::Detail => {
            let title = detail_title(app, m);
            let lines = detail(app, m);
            scroll(frame, app, body, Some(&title), lines);
        }
        Screen::Actions => render_actions(frame, app, m, body, compact),
        Screen::Form => render_form(frame, app, m, body, compact),
        Screen::Confirm | Screen::Discard => render_confirmation(frame, app, m, body, compact),
        Screen::Busy => render_busy(frame, app, m, body),
        Screen::Failed => render_failed(frame, app, m, body),
        Screen::Receipt => render_receipt(frame, app, m, body),
        Screen::Help => {
            let lines = help_lines(app, m);
            scroll(frame, app, body, Some(m.facts), lines);
        }
    }
    super::chrome::footer(frame, app, m, regions.footer);
}

/// The screen a popup sits on. Drawing it and then flattening it to one shade keeps the user's
/// place in view without letting it compete with the card in front.
fn underlay(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
    compact: bool,
) {
    let scroll = app.max_scroll;
    if app.form.is_some() {
        render_form(frame, app, m, area, compact);
    } else {
        render_page(frame, app, m, area, compact);
    }
    app.max_scroll = scroll;
    dim(frame.buffer_mut(), area);
}

/// A numbered step whose wrapped rows hang under the text rather than under the number.
fn step_lines(index: usize, text: &str, width: usize) -> Lines {
    const INDENT: usize = 3;
    wrapped(vec![muted(text)], width.saturating_sub(INDENT))
        .into_iter()
        .enumerate()
        .map(|(row, line)| {
            let mut spans = vec![Span::styled(
                if row == 0 {
                    format!("{}  ", index + 1)
                } else {
                    " ".repeat(INDENT)
                },
                Style::default().fg(theme::border()),
            )];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// The three things a first World takes, in order.
fn setup_steps(m: &i18n::WorldMessages, width: usize) -> Lines {
    [m.step_hub, m.step_create, m.step_join]
        .iter()
        .enumerate()
        .flat_map(|(index, step)| step_lines(index, step, width))
        .collect()
}

/// A headline sentence with its judgement symbol; returns the area below it.
fn headline(frame: &mut Frame<'_>, area: Rect, tone: Tone, text: &str) -> Rect {
    if area.height == 0 || area.width == 0 {
        return area;
    }
    let mut line = judged(tone.symbol(), tone.color(), text);
    if matches!(tone, Tone::Warn | Tone::Fail) {
        for span in &mut line.spans[1..] {
            span.style = span.style.add_modifier(Modifier::BOLD);
        }
    }
    let lines = wrapped(vec![line], usize::from(area.width));
    let rows = (lines.len() as u16)
        .min(area.height.saturating_sub(1))
        .max(1);
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            height: rows,
            ..area
        },
    );
    Rect {
        y: area.y + rows + 1,
        height: area.height.saturating_sub(rows + 1),
        ..area
    }
}

fn render_page(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
    compact: bool,
) {
    app.viewport_rows = area.height.saturating_sub(2).max(1) as usize;
    if app.page == Page::Hub {
        render_hub(frame, app, m, area);
        return;
    }
    let Some(snapshot) = &app.snapshot else {
        if let Some(failure) = &app.load_error {
            let lines = failure_lines(failure, m);
            notice(frame, area, Tone::Fail, m.error, lines);
        } else {
            notice(frame, area, Tone::Info, m.loading, Vec::new());
        }
        return;
    };
    if let ReadState::Failed(failure) = &snapshot.config {
        let lines = failure_lines(failure, m);
        let title = if failure.kind == FailureKind::Permission {
            m.permission
        } else {
            m.error
        };
        notice(frame, area, Tone::Fail, title, lines);
        return;
    }
    if matches!(snapshot.config, ReadState::Missing) {
        render_welcome(frame, app, m, area, compact);
        return;
    }
    match app.page {
        Page::Overview => render_overview(frame, app, m, area),
        Page::Machines => render_machines(frame, app, m, area, compact),
        Page::Grants => render_grants(frame, app, m, area),
        Page::Connection => super::status_pages::connection(frame, app, m, area),
        Page::Events => render_events(frame, app, m, area),
        Page::Service => super::status_pages::service(frame, app, m, area),
        Page::Hub => unreachable!(),
    }
}

fn render_welcome(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
    compact: bool,
) {
    let mut intro = wrapped(
        vec![strong(m.welcome), plain(m.welcome_body)],
        usize::from(area.width),
    );
    let option_rows = if compact { 1 } else { 2 };
    let options_height = super::WELCOME_ACTIONS.len() as u16 * option_rows;
    let room = area.height.saturating_sub(options_height + 1);
    // The body is one paragraph: keep all of it or none, because a sentence cut off halfway
    // reads as a rendering fault rather than as a short screen.
    if intro.len() as u16 > room {
        intro.truncate(wrapped(vec![strong(m.welcome)], usize::from(area.width)).len());
    }
    let intro_height = (intro.len() as u16).min(room);
    let mut remaining = area
        .height
        .saturating_sub(intro_height + 1 + options_height);
    let sketch_rows = topology::sketch_height();
    let sketch_height = if !compact && area.width >= 48 && remaining > sketch_rows {
        remaining -= sketch_rows + 1;
        sketch_rows
    } else {
        0
    };
    let steps = if compact {
        Vec::new()
    } else {
        setup_steps(m, usize::from(area.width))
    };
    let steps_height = if remaining > steps.len() as u16 {
        steps.len() as u16 + 1
    } else {
        0
    };
    let parts = Layout::vertical([
        Constraint::Length(intro_height),
        Constraint::Length(1),
        Constraint::Length(sketch_height + u16::from(sketch_height > 0)),
        Constraint::Length(options_height),
        Constraint::Length(steps_height),
        Constraint::Min(0),
    ])
    .split(area);
    frame.render_widget(Paragraph::new(intro), parts[0]);
    if sketch_height > 0 {
        topology::sketch(
            frame.buffer_mut(),
            Rect {
                height: sketch_height,
                ..parts[2]
            },
            m,
        );
    }
    let selected = app.selected[app.page.index()].min(super::WELCOME_ACTIONS.len() - 1);
    let mut lines = Vec::new();
    for (index, action) in super::WELCOME_ACTIONS.iter().enumerate() {
        let focused = index == selected;
        let hint = match action {
            MenuAction::Create => m.welcome_create_hint,
            MenuAction::Join => m.welcome_join_hint,
            _ => m.welcome_hub_hint,
        };
        let row_width = usize::from(parts[3].width);
        let title = format!("{} {}", if focused { "▌" } else { " " }, action.label(m));
        let title_style = if focused {
            selection_style()
        } else {
            Style::default()
                .fg(theme::fg())
                .add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(Span::styled(
            padded(&title, row_width),
            title_style,
        )));
        if option_rows == 2 {
            let hint_style = if focused {
                selection_style().remove_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::muted())
            };
            lines.push(Line::from(Span::styled(
                padded(
                    &format!("{}   {hint}", if focused { "▌" } else { " " }),
                    row_width,
                ),
                hint_style,
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines), parts[3]);
    if steps_height > 0 {
        frame.render_widget(
            Paragraph::new(steps),
            Rect {
                y: parts[4].y + 1,
                height: parts[4].height - 1,
                ..parts[4]
            },
        );
    }
}

/// The picture the Overview draws, from the live member list or, when the node is down, from
/// the enrollment alone so the user still sees where this machine stands.
fn build_topology(app: &WorldApp, m: &i18n::WorldMessages) -> Topology {
    let snapshot = app.snapshot.as_ref();
    let config = snapshot.and_then(|snapshot| snapshot.config.get());
    let status = snapshot.and_then(current_status);
    let live = snapshot.is_some_and(|snapshot| matches!(snapshot.node, ReadState::Ready(_)))
        && status.is_some_and(|(_, fresh)| fresh);
    let hub = Hub {
        address: config
            .map(|config| config.hub.join(", "))
            .unwrap_or_else(|| m.unknown.to_string()),
        link: status
            .filter(|_| live)
            .map(|(status, _)| model::LinkTone::from_state(&status.link.state)),
    };
    let mut machines = app
        .nodes()
        .iter()
        .map(|node| Machine {
            name: node.name.clone(),
            presence: if live {
                Presence::from_state(&node.state)
            } else {
                Presence::Unknown
            },
            system: format!("{} · {}", os_label(&node.os), node.arch),
            is_self: node.is_self,
            rtt_ms: node.hub_rtt_ms,
            last_seen: relative(&node.last_seen, m),
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
    let topology = build_topology(app, m);
    let body = headline(frame, area, verdict.tone, &verdict.text);
    let strip_rows = if body.height >= 15 { 3 } else { 0 };
    let diagram = topology::diagram(
        &topology,
        m,
        body.width,
        body.height.saturating_sub(strip_rows),
    );
    let parts = Layout::vertical([
        Constraint::Length(diagram.height),
        Constraint::Length(strip_rows),
        Constraint::Min(0),
    ])
    .split(body);
    topology::render(frame.buffer_mut(), parts[0], &topology, m, &diagram);
    if strip_rows == 0 {
        return;
    }
    let strip = Rect {
        y: parts[1].y + 1,
        height: parts[1].height - 1,
        ..parts[1]
    };
    let lines = if topology.machines.len() == 1 && app.nodes().len() <= 1 {
        vec![muted(m.only_self)]
    } else if let Some(node) = app.focused_node() {
        vec![
            pair(
                m.note,
                node.effective_note()
                    .map(safe)
                    .unwrap_or_else(|| m.none.to_string()),
            ),
            Line::from(vec![
                Span::styled(
                    format!("{}  ", m.activity),
                    Style::default().fg(theme::muted()),
                ),
                Span::styled(
                    activity_line(node, m).unwrap_or_else(|| m.unknown.to_string()),
                    Style::default().fg(theme::fg()),
                ),
                Span::styled(
                    format!("    {}  ", m.last_seen),
                    Style::default().fg(theme::muted()),
                ),
                Span::styled(
                    relative(&node.last_seen, m),
                    Style::default().fg(theme::fg()),
                ),
            ]),
        ]
    } else {
        Vec::new()
    };
    frame.render_widget(
        Paragraph::new(wrapped(lines, usize::from(strip.width))),
        strip,
    );
}

fn presence_cell(presence: Presence, m: &i18n::WorldMessages) -> Cell<'static> {
    Cell::from(Span::styled(
        format!("{} {}", presence.symbol(), presence.label(m)),
        Style::default().fg(presence.color()),
    ))
}

fn render_machines(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
    compact: bool,
) {
    if let Some(lines) = node_error(app, m, true) {
        notice(frame, area, Tone::Fail, m.unavailable, lines);
        return;
    }
    if app.nodes().is_empty() {
        notice(frame, area, Tone::Info, m.empty, Vec::new());
        return;
    }
    let wide = !compact && area.width >= 100;
    let panes = Layout::horizontal(if wide {
        vec![Constraint::Percentage(56), Constraint::Percentage(44)]
    } else {
        vec![Constraint::Percentage(100)]
    })
    .spacing(if wide { 3 } else { 0 })
    .split(area);
    let list = panes[0];
    title_rule(
        frame,
        list,
        &format!("{} · {}", m.machines, app.nodes().len()),
        None,
    );
    let names = app
        .nodes()
        .iter()
        .map(|node| {
            let mut name = safe(&node.name);
            if node.is_self {
                name.push_str(&format!(" · {}", m.this_machine));
            }
            name
        })
        .collect::<Vec<_>>();
    let name_width = names
        .iter()
        .map(|name| width(name))
        .max()
        .unwrap_or(8)
        .clamp(8, 32);
    let systems = app
        .nodes()
        .iter()
        .map(|node| format!("{} · {}", os_label(&node.os), node.arch))
        .collect::<Vec<_>>();
    let system_width = systems
        .iter()
        .map(|system| width(system))
        .max()
        .unwrap_or(6)
        .clamp(6, 22);
    let state_width = [m.online, m.offline, m.revoked, m.unknown]
        .iter()
        .map(|label| width(label) + 2)
        .max()
        .unwrap_or(6);
    // A narrow terminal keeps state, name and latency; a note column too narrow to read is
    // noise, and the detail pane carries the note anyway.
    let with_system = list.width >= 60;
    let fixed = if with_system {
        vec![state_width, name_width, system_width, 10]
    } else {
        vec![state_width, name_width]
    };
    let (mut constraints, note_width) = super::widgets::columns(list.width, &fixed);
    let with_note = with_system && (note_width >= 10 || !wide);
    if with_system && !with_note {
        constraints.pop();
    }
    let selected = app.selected[Page::Machines.index()];
    let table_area = Rect {
        y: list.y + 1,
        height: list.height.saturating_sub(1),
        ..list
    };
    let window = table_window(app.nodes().len(), selected, table_area.height);
    let rows = app
        .nodes()
        .iter()
        .zip(names.iter())
        .zip(systems.iter())
        .skip(window.start)
        .take(window.len())
        .map(|((node, name), system)| {
            let presence = Presence::from_state(&node.state);
            let latency = if presence == Presence::Online {
                node.hub_rtt_ms
                    .map(|rtt| format!("{rtt} ms"))
                    .unwrap_or_else(|| m.unknown.to_string())
            } else {
                relative(&node.last_seen, m)
            };
            let note = node
                .effective_note()
                .map(safe)
                .or_else(|| activity_line(node, m))
                .unwrap_or_default();
            let mut cells = vec![
                presence_cell(presence, m),
                Cell::from(Span::styled(
                    clipped(name, name_width),
                    Style::default()
                        .fg(theme::fg())
                        .add_modifier(Modifier::BOLD),
                )),
            ];
            if with_system {
                cells.push(Cell::from(Span::styled(
                    clipped(system, system_width),
                    Style::default().fg(theme::muted()),
                )));
            }
            cells.push(Cell::from(Span::styled(
                clipped(&latency, if with_system { 10 } else { note_width }),
                Style::default().fg(theme::fg()),
            )));
            if with_note {
                cells.push(Cell::from(Span::styled(
                    clipped(&note, note_width),
                    Style::default().fg(theme::muted()),
                )));
            }
            Row::new(cells)
        })
        .collect::<Vec<_>>();
    let mut header = vec![m.col_state, m.name];
    if with_system {
        header.push(m.col_system);
    }
    header.push(m.col_latency);
    if with_note {
        header.push(m.note);
    }
    table(
        frame,
        app,
        table_area,
        &header,
        rows,
        &constraints,
        selected.saturating_sub(window.start),
    );
    if wide && let Some(node) = app.focused_node() {
        let pane = panes[1];
        let body = Rect {
            y: pane.y + 2,
            height: pane.height.saturating_sub(2),
            ..pane
        };
        let lines = wrapped(node_details(node, m), usize::from(pane.width));
        // The pane is a preview, not the record: say where the rest is instead of ending
        // mid-fact with no way of knowing anything was left out.
        let more = lines.len() > usize::from(body.height);
        title_rule(
            frame,
            pane,
            &safe(&node.name),
            more.then(|| format!("F1 {}", m.facts)).as_deref(),
        );
        frame.render_widget(Paragraph::new(lines), body);
    }
}

fn render_grants(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    if let Some(lines) = node_error(app, m, false) {
        notice(frame, area, Tone::Fail, m.unavailable, lines);
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
        let title = if node.grants.revision == 0 {
            m.grants_default
        } else {
            m.grants_empty
        };
        notice(frame, area, Tone::Info, title, vec![muted(m.grants_help)]);
        return;
    }
    title_rule(
        frame,
        area,
        &format!("{} · {}", m.grants, node.grants.grants.len()),
        Some(&format!("{} {}", m.grant_revision, node.grants.revision)),
    );
    let measure = |values: &mut dyn Iterator<Item = String>, floor: usize, ceiling: usize| {
        values
            .map(|value| width(&value))
            .max()
            .unwrap_or(floor)
            .clamp(floor, ceiling)
    };
    let principal_width = measure(
        &mut node
            .grants
            .grants
            .iter()
            .map(|entry| safe(&entry.grant.principal)),
        width(m.principal),
        28,
    );
    let targets_width = measure(
        &mut node
            .grants
            .grants
            .iter()
            .map(|entry| safe(&entry.grant.nodes.join(", "))),
        width(m.targets),
        32,
    );
    let verbs_width = measure(
        &mut node
            .grants
            .grants
            .iter()
            .map(|entry| safe(&entry.grant.verbs.join(", "))),
        width(m.verbs),
        32,
    );
    // Narrow terminals keep who and where; the detail view has the rest.
    let full = area.width >= 72;
    let fixed = if full {
        vec![principal_width, targets_width, verbs_width]
    } else {
        vec![principal_width]
    };
    let (constraints, last_width) = super::widgets::columns(area.width, &fixed);
    let rows_len = node.grants.grants.len();
    let selected = app.selected[Page::Grants.index()];
    let table_area = Rect {
        y: area.y + 1,
        height: area
            .height
            .saturating_sub(if area.height >= 8 { 3 } else { 1 }),
        ..area
    };
    let window = table_window(rows_len, selected, table_area.height);
    let rows = node
        .grants
        .grants
        .iter()
        .skip(window.start)
        .take(window.len())
        .map(|entry| {
            let mut cells = vec![
                Cell::from(Span::styled(
                    clipped(&safe(&entry.grant.principal), principal_width),
                    Style::default()
                        .fg(theme::fg())
                        .add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    clipped(
                        &safe(&entry.grant.nodes.join(", ")),
                        if full { targets_width } else { last_width },
                    ),
                    Style::default().fg(theme::fg()),
                )),
            ];
            if full {
                cells.push(Cell::from(Span::styled(
                    clipped(&safe(&entry.grant.verbs.join(", ")), verbs_width),
                    Style::default().fg(theme::fg()),
                )));
                cells.push(Cell::from(Span::styled(
                    clipped(entry.grant.expires.as_deref().unwrap_or(m.none), last_width),
                    Style::default().fg(theme::muted()),
                )));
            }
            Row::new(cells)
        })
        .collect::<Vec<_>>();
    let header: Vec<&str> = if full {
        vec![m.principal, m.targets, m.verbs, m.expires]
    } else {
        vec![m.principal, m.targets]
    };
    table(
        frame,
        app,
        table_area,
        &header,
        rows,
        &constraints,
        selected.saturating_sub(window.start),
    );
    if area.height >= 8 {
        let used = 2 + (rows_len as u16).min(table_area.height.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(wrapped(vec![muted(m.grants_help)], usize::from(area.width))),
            Rect::new(
                area.x,
                (area.y + used + 1).min(area.bottom() - 2),
                area.width,
                2,
            ),
        );
    }
}

fn render_events(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let status = if app.event_error.is_some() {
        m.error
    } else if app.event_cursor < app.event_latest {
        m.more_events
    } else if app.events_loaded {
        m.caught_up
    } else {
        m.loading
    };
    if app.events.is_empty() {
        let mut lines = Vec::new();
        // One failure, said once: the event read and the node read usually fail for the same
        // reason, and printing both spells the same sentence out twice.
        if let Some(error) = &app.event_error {
            lines.extend(failure_lines(error, m));
        } else if let Some(error) = node_error(app, m, false) {
            lines.extend(error);
        }
        if app.event_gap {
            lines.push(warn(m.event_gap));
        }
        lines.push(pair(m.cursor, app.event_cursor));
        let tone = if app.event_error.is_some() {
            Tone::Fail
        } else {
            Tone::Info
        };
        notice(frame, area, tone, status, lines);
        return;
    }
    title_rule(
        frame,
        area,
        &format!("{} · {}", m.events, app.events.len()),
        Some(status),
    );
    let gap = u16::from(app.event_gap);
    if app.event_gap {
        frame.render_widget(
            Paragraph::new(judged(Tone::Warn.symbol(), Tone::Warn.color(), m.event_gap)),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }
    let label_width = app
        .events
        .iter()
        .map(|event| width(model::event_label(&event.kind, m).0) + 2)
        .max()
        .unwrap_or(10)
        .clamp(10, 30);
    let (constraints, subject_width) = super::widgets::columns(area.width, &[12, label_width]);
    let selected = app.selected[Page::Events.index()];
    let table_area = Rect {
        y: area.y + 1 + gap,
        height: area.height.saturating_sub(1 + gap),
        ..area
    };
    let window = table_window(app.events.len(), selected, table_area.height);
    let rows = app
        .events
        .iter()
        .skip(window.start)
        .take(window.len())
        .map(|event| {
            let (label, tone) = model::event_label(&event.kind, m);
            Row::new(vec![
                Cell::from(Span::styled(
                    relative(&event.at, m),
                    Style::default().fg(theme::muted()),
                )),
                Cell::from(Line::from(vec![
                    Span::styled(
                        format!("{} ", tone.symbol()),
                        Style::default()
                            .fg(tone.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(label.to_string(), Style::default().fg(theme::fg())),
                ])),
                Cell::from(Span::styled(
                    clipped(&safe(&event.subject), subject_width),
                    Style::default()
                        .fg(theme::fg())
                        .add_modifier(Modifier::BOLD),
                )),
            ])
        })
        .collect::<Vec<_>>();
    table(
        frame,
        app,
        table_area,
        &[m.col_when, m.col_event, m.subject],
        rows,
        &constraints,
        selected.saturating_sub(window.start),
    );
}

fn render_hub(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let directory = app.hub_data.to_string_lossy().into_owned();
    let state = app.snapshot.as_ref().map(|snapshot| &snapshot.hub);
    match state {
        Some(ReadState::Ready(hub)) => {
            let members = hub.members.len();
            let live = crate::world::hub::status_is_live(hub);
            let (tone, title) = if live {
                (Tone::Ok, m.hub_running.to_string())
            } else {
                (
                    Tone::Warn,
                    format!("{} · {}", m.hub_not_running, relative(&hub.written_at, m)),
                )
            };
            let body = headline(frame, area, tone, &title);
            let tiles_height = if body.height >= 8 { 3 } else { 0 };
            let parts = Layout::vertical([
                Constraint::Length(tiles_height),
                Constraint::Min(1),
                Constraint::Length(u16::from(body.height >= 6)),
            ])
            .split(body);
            if tiles_height > 0 {
                let tiles = [
                    Tile::new(m.machines, hub.members.len(), theme::accent()),
                    Tile::new(m.pending_invites, hub.open_invites, theme::accent()),
                    Tile::new(m.data_connections, hub.data_connections, theme::accent()),
                    Tile::new(m.events, hub.events, theme::fg()),
                    Tile::new(m.listen, safe(&hub.listen), theme::fg()),
                    Tile::new(m.tls, safe(&hub.tls), theme::fg()),
                ];
                stat_tiles(frame, parts[0], &tiles);
            }
            if hub.members.is_empty() {
                notice(frame, parts[1], Tone::Info, m.empty, Vec::new());
            } else {
                title_rule(frame, parts[1], m.machines, None);
                let name_width = hub
                    .members
                    .iter()
                    .map(|member| width(&safe(&member.name)))
                    .max()
                    .unwrap_or(8)
                    .clamp(8, 32);
                let selected = app.selected[Page::Hub.index()];
                let table_area = Rect {
                    y: parts[1].y + 1,
                    height: parts[1].height.saturating_sub(1),
                    ..parts[1]
                };
                let window = table_window(hub.members.len(), selected, table_area.height);
                let rows = hub
                    .members
                    .iter()
                    .skip(window.start)
                    .take(window.len())
                    .map(|member| {
                        let presence = Presence::from_state(&member.state);
                        Row::new(vec![
                            presence_cell(presence, m),
                            Cell::from(Span::styled(
                                clipped(&safe(&member.name), name_width),
                                Style::default()
                                    .fg(theme::fg())
                                    .add_modifier(Modifier::BOLD),
                            )),
                            Cell::from(Span::styled(
                                relative(&member.last_seen, m),
                                Style::default().fg(theme::fg()),
                            )),
                            Cell::from(Span::styled(
                                member.outbox.to_string(),
                                Style::default().fg(if member.outbox == 0 {
                                    theme::muted()
                                } else {
                                    theme::warning()
                                }),
                            )),
                            Cell::from(Span::styled(
                                safe(&member.version),
                                Style::default().fg(theme::muted()),
                            )),
                        ])
                    })
                    .collect::<Vec<_>>();
                let (constraints, _) = super::widgets::columns(
                    parts[1].width,
                    &[
                        12,
                        name_width,
                        width(m.last_seen).max(12),
                        width(m.outbox).max(6),
                    ],
                );
                table(
                    frame,
                    app,
                    table_area,
                    &[m.col_state, m.name, m.last_seen, m.outbox, m.version],
                    rows,
                    &constraints,
                    selected.saturating_sub(window.start),
                );
            }
            if parts[2].height > 0 {
                // Right under the member list, not at the foot of the page: a footnote adrift
                // in empty space reads as something the layout forgot.
                let used = if members == 0 {
                    parts[1].height
                } else {
                    (2 + members as u16).min(parts[1].height)
                };
                let y = (parts[1].y + used + 1).min(parts[2].y);
                frame.render_widget(
                    Paragraph::new(pair(m.directory, &directory)),
                    Rect { y, ..parts[2] },
                );
            }
        }
        Some(ReadState::Missing) => notice(
            frame,
            area,
            Tone::Info,
            m.hub_absent,
            vec![muted(m.hub_help), plain(""), pair(m.directory, &directory)],
        ),
        Some(ReadState::Failed(error)) => {
            let mut lines = failure_lines(error, m);
            lines.push(plain(""));
            lines.push(pair(m.directory, &directory));
            let title = if error.kind == FailureKind::Permission {
                m.permission
            } else {
                m.error
            };
            notice(frame, area, Tone::Fail, title, lines);
        }
        None => {
            let lines = app
                .load_error
                .as_ref()
                .map(|error| failure_lines(error, m))
                .unwrap_or_default();
            notice(frame, area, Tone::Info, m.loading, lines);
        }
    }
}

fn render_actions(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
    compact: bool,
) {
    underlay(frame, app, m, area, compact);
    let recommended = app.recommended();
    let actions = app.actions();
    let labels = actions
        .iter()
        .map(|action| action.label(m).to_string())
        .collect::<Vec<_>>();
    let label_width = labels.iter().map(|label| width(label)).max().unwrap_or(10);
    let tag_width = width(m.recommended) + 3;
    let inner_width = (label_width + tag_width + 8).clamp(30, 72);
    let title = if app.page.lists_machines() && app.enrolled() {
        app.focused_node()
            .map(|node| format!("{} · {}", m.actions, safe(&node.name)))
            .unwrap_or_else(|| m.actions.to_string())
    } else {
        format!("{} · {}", m.actions, app.page.title(m))
    };
    let inner = popup(
        frame,
        area,
        inner_width as u16 + 4,
        actions.len() as u16 + 4,
        &title,
        theme::border(),
    );
    let mut lines = vec![Line::raw("")];
    let row_width = usize::from(inner.width);
    for (index, action) in actions.iter().enumerate() {
        let focused = index == app.menu_selected;
        let hotkey = action
            .hotkey()
            .map(|key| key.to_string())
            .unwrap_or_default();
        let mut text = format!("{} {}", if focused { "▌" } else { " " }, labels[index]);
        if matches!(
            action,
            MenuAction::ServicePage | MenuAction::ConnectionPage | MenuAction::HubPage
        ) {
            text.push_str(" ›");
        }
        if recommended == Some(*action) {
            text.push_str(&format!("  · {}", m.recommended));
        }
        let text = padded(&text, row_width.saturating_sub(hotkey.len() + 1));
        let style = if focused {
            selection_style()
        } else {
            Style::default().fg(theme::fg())
        };
        lines.push(Line::from(vec![
            Span::styled(text, style),
            Span::styled(
                format!("{hotkey} "),
                if focused {
                    style
                } else {
                    Style::default().fg(theme::muted())
                },
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn input_line(field: &super::forms::Field, focused: bool, width: usize) -> Line<'static> {
    let ground = Style::default().fg(theme::fg()).bg(theme::surface());
    let mut spans = vec![Span::styled(
        if focused { "▌" } else { " " },
        Style::default()
            .fg(if focused {
                theme::accent()
            } else {
                theme::surface()
            })
            .bg(theme::surface()),
    )];
    let cells = field.input.cells(width.saturating_sub(2));
    for cell in cells {
        let mut style = ground;
        if cell.selected || (cell.cursor && focused) {
            style = style.add_modifier(Modifier::REVERSED);
        }
        spans.push(Span::styled(cell.text, style));
    }
    let used = spans.iter().map(Span::width).sum::<usize>();
    spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), ground));
    if theme::flat() {
        for span in &mut spans {
            span.style = span.style.add_modifier(Modifier::UNDERLINED);
        }
    }
    Line::from(spans)
}

fn choice_line(
    field: &super::forms::Field,
    focused: bool,
    m: &i18n::WorldMessages,
) -> Line<'static> {
    let options: Vec<(&str, &str)> = if field.kind == FieldKind::Network {
        vec![("auto", "auto"), ("direct", "direct"), ("system", "system")]
    } else {
        vec![("yes", m.yes), ("no", m.no)]
    };
    let mut spans = vec![Span::raw("  ")];
    for (value, label) in options {
        let active = field.input.text() == value;
        let style = if active {
            let style = Style::default()
                .fg(theme::accent())
                .bg(if focused {
                    theme::highlight()
                } else {
                    theme::surface()
                })
                .add_modifier(Modifier::BOLD);
            if theme::flat() {
                style.add_modifier(Modifier::UNDERLINED)
            } else {
                style
            }
        } else {
            Style::default().fg(theme::muted())
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw("  "));
    }
    if focused {
        spans.push(Span::styled(
            m.switch_hint,
            Style::default().fg(theme::border()),
        ));
    }
    Line::from(spans)
}

/// How much of itself a form explains: the guide pane takes the job when there is one, a form
/// with room explains every field so the page never shifts under the cursor, and a short
/// terminal falls back to explaining only the field the cursor is on.
#[derive(Clone, Copy, Eq, PartialEq)]
enum FieldHelp {
    None,
    Focused,
    Every,
}

/// The form as lines, with the row each focus target starts on so the caller can scroll to it.
fn form_body(
    form: &super::forms::Form,
    m: &i18n::WorldMessages,
    width: usize,
    compact: bool,
    help: FieldHelp,
) -> (Lines, Vec<(Focus, usize)>) {
    let row_width = width;
    let input_width = width;
    let focus = form.focus();
    let error_field = form.error_field();
    let mut lines: Lines = vec![strong(form.title(m))];
    if !compact && let Some(subtitle) = form.subtitle(m) {
        lines.extend(wrapped(vec![muted(subtitle)], row_width));
    }
    lines.push(Line::raw(""));
    let mut anchors: Vec<(Focus, usize)> = Vec::new();
    for target in form.targets() {
        anchors.push((target, lines.len()));
        match target {
            Focus::Field(index) => {
                let field = &form.fields[index];
                let focused = focus == target;
                let mut label = vec![Span::styled(
                    field.kind.label(m, &form.kind).to_string(),
                    if focused {
                        Style::default()
                            .fg(theme::accent())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme::fg())
                    },
                )];
                if field.kind.required() {
                    label.push(Span::styled(" *", Style::default().fg(theme::warning())));
                }
                if !field.kind.is_choice()
                    && field.input.limit() <= 512
                    && (focused || field.input.len() > 0)
                {
                    label.push(Span::styled(
                        format!("   {}/{}", field.input.len(), field.input.limit()),
                        Style::default().fg(theme::border()),
                    ));
                }
                lines.push(Line::from(label));
                if field.kind.is_choice() {
                    lines.push(choice_line(field, focused, m));
                } else {
                    lines.push(input_line(field, focused, input_width));
                }
                let complaint = match &field.input.error {
                    Some(InputError::TooLong { limit, count }) => Some(i18n::fill(
                        m.too_long,
                        &[("limit", &limit.to_string()), ("count", &count.to_string())],
                    )),
                    Some(InputError::SingleLine) => Some(m.single_line.to_string()),
                    None if error_field == Some(field.kind) && form.error.is_some() => {
                        Some(form.error_text(m))
                    }
                    None => None,
                };
                if let Some(complaint) = complaint {
                    lines.extend(wrapped(
                        vec![Line::from(Span::styled(
                            format!("× {complaint}"),
                            Style::default().fg(theme::danger()),
                        ))],
                        row_width,
                    ));
                } else if help == FieldHelp::Every || (help == FieldHelp::Focused && focused) {
                    lines.extend(elided(
                        wrapped(vec![muted(field.kind.help(m, &form.kind))], row_width),
                        if compact { 1 } else { 3 },
                        row_width,
                    ));
                }
                lines.push(Line::raw(""));
            }
            Focus::Advanced => {
                let focused = focus == target;
                let text = format!(
                    "{} {}{}",
                    if form.advanced { "▼" } else { "►" },
                    m.advanced,
                    if form.advanced || compact {
                        String::new()
                    } else {
                        format!("  ·  {}", m.advanced_hint)
                    }
                );
                lines.push(Line::from(Span::styled(
                    padded(&text, row_width),
                    if focused {
                        selection_style()
                    } else {
                        Style::default().fg(theme::muted())
                    },
                )));
                lines.push(Line::raw(""));
            }
            Focus::Submit => {
                let focused = focus == target;
                let mut spans = vec![button(form.submit_label(m), focused, theme::accent())];
                if focused {
                    spans.push(Span::styled(
                        "   Enter",
                        Style::default().fg(theme::border()),
                    ));
                }
                lines.push(Line::from(spans));
                if !compact && form.fields.iter().any(|field| field.kind.required()) {
                    lines.push(Line::raw(""));
                    lines.push(muted(m.required_legend));
                }
            }
        }
    }
    if form.error.is_some() && error_field.is_none() {
        let row = anchors.last().map_or(lines.len(), |(_, row)| *row);
        for (offset, line) in wrapped(
            vec![Line::from(Span::styled(
                format!("× {}", form.error_text(m)),
                Style::default().fg(theme::danger()),
            ))],
            row_width,
        )
        .into_iter()
        .enumerate()
        {
            lines.insert(row + offset, line);
        }
    }
    (lines, anchors)
}

fn render_form(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
    compact: bool,
) {
    let Some(form) = &app.form else {
        return;
    };
    let guide = !compact && area.width >= 96;
    let panes = Layout::horizontal(if guide {
        vec![Constraint::Min(40), Constraint::Length(36)]
    } else {
        vec![Constraint::Percentage(100)]
    })
    .spacing(if guide { 4 } else { 0 })
    .split(area);
    let column = panes[0];
    // One measure for the whole form: label, input, help and complaint draw in the same column,
    // so the eye follows a single edge down the page.
    let form_width = usize::from(column.width).min(64);
    let wanted = if guide {
        FieldHelp::None
    } else {
        FieldHelp::Every
    };
    let (mut lines, mut anchors) = form_body(form, m, form_width, compact, wanted);
    // Explaining every field is what stops the page shifting as the cursor moves, but seeing
    // the whole form at once is worth more.
    if wanted == FieldHelp::Every && lines.len() > usize::from(column.height) {
        (lines, anchors) = form_body(form, m, form_width, compact, FieldHelp::Focused);
    }
    let focus = form.focus();
    let anchor = anchors
        .iter()
        .find(|(target, _)| *target == focus)
        .map_or(0, |(_, row)| *row);
    let visible = usize::from(column.height);
    let offset = if anchor + 4 > visible {
        (anchor + 4 - visible).min(lines.len().saturating_sub(visible))
    } else {
        0
    };
    frame.render_widget(
        Paragraph::new(lines).scroll((offset.min(u16::MAX as usize) as u16, 0)),
        column,
    );
    if guide {
        let pane = panes[1];
        title_rule(frame, pane, m.guide, None);
        let mut help: Lines = Vec::new();
        if let Focus::Field(index) = focus {
            help.push(plain(form.fields[index].kind.help(m, &form.kind)));
            help.push(Line::raw(""));
        }
        match form.kind {
            FormKind::Create | FormKind::Join => {
                help.extend(setup_steps(m, usize::from(pane.width)))
            }
            FormKind::Invite => help.push(muted(m.invite_help)),
            FormKind::Grant(_) => {
                help.push(muted(m.grants_help));
                help.push(Line::raw(""));
                help.push(muted(m.grants_default));
            }
            _ => {}
        }
        frame.render_widget(
            Paragraph::new(wrapped(help, usize::from(pane.width))),
            Rect {
                y: pane.y + 2,
                height: pane.height.saturating_sub(2),
                ..pane
            },
        );
    }
}

fn destructive(action: &Action) -> bool {
    matches!(
        action,
        Action::Revoke { .. }
            | Action::Leave
            | Action::DeleteGrant { .. }
            | Action::HubRevoke { .. }
            | Action::Service {
                action: ServiceAction::Stop | ServiceAction::Uninstall
            }
    )
}

fn render_confirmation(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
    compact: bool,
) {
    let discard = app.screen == Screen::Discard;
    let (title, body, color) = if discard {
        (
            m.discard.to_string(),
            vec![plain(m.discard_body)],
            theme::warning(),
        )
    } else {
        let title = app
            .preview
            .as_ref()
            .map(|action| action_name(action, m).to_string())
            .unwrap_or_else(|| m.preview.to_string());
        let color = if app.preview.as_ref().is_some_and(destructive) {
            theme::danger()
        } else {
            theme::accent()
        };
        (title, preview(app, m), color)
    };
    let no = if discard { m.keep_editing } else { m.cancel };
    let yes = if discard { m.discard } else { m.confirm };
    let buttons = |confirm_yes: bool| {
        Line::from(vec![
            button(no, !confirm_yes, theme::muted()),
            Span::raw("     "),
            button(yes, confirm_yes, color),
        ])
        .alignment(Alignment::Center)
    };
    if compact || area.width < 50 || area.height < 10 {
        let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
        scroll(frame, app, parts[0], Some(&title), body);
        frame.render_widget(Paragraph::new(buttons(app.confirm_yes)), parts[1]);
        return;
    }
    underlay(frame, app, m, area, compact);
    let width = area.width.min(74);
    let text = wrapped(body, usize::from(width) - 6);
    let height = (text.len() as u16 + 6).min(area.height);
    let inner = popup(frame, area, width, height, &title, color);
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    scroll(frame, app, parts[1], None, text);
    frame.render_widget(Paragraph::new(buttons(app.confirm_yes)), parts[3]);
}

fn centered(frame: &mut Frame<'_>, area: Rect, lines: Lines) {
    let width = usize::from(area.width).saturating_sub(4).min(72);
    let lines = wrapped(lines, width);
    let height = (lines.len() as u16).min(area.height);
    let top = area.y + area.height.saturating_sub(height) / 3;
    let left = area.x + (area.width.saturating_sub(width as u16)) / 2;
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(left, top, width as u16, height),
    );
}

fn render_busy(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let dots = ["●○○", "○●○", "○○●"][(app.frame / 3 % 3) as usize];
    let seconds = app
        .worker
        .as_ref()
        .map(|worker| worker.started.elapsed().as_secs())
        .unwrap_or(0);
    let action = app
        .active
        .as_ref()
        .map(|action| action_name(action, m))
        .unwrap_or(m.busy);
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{dots} "),
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{action} · {seconds} s"),
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        muted(m.busy_body),
        Line::raw(""),
        warn(m.running_exit),
    ];
    centered(frame, area, lines);
}

fn render_failed(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let Some(failure) = app.failure.clone() else {
        notice(frame, area, Tone::Fail, m.failed, Vec::new());
        return;
    };
    let action = app
        .active
        .as_ref()
        .or(app.preview.as_ref())
        .map(|action| format!(" · {}", action_name(action, m)))
        .unwrap_or_default();
    let body = headline(frame, area, Tone::Fail, &format!("{}{action}", m.failed));
    let lines = classified_failure_lines(&failure, m);
    scroll(frame, app, body, None, lines);
}

fn render_receipt(frame: &mut Frame<'_>, app: &mut WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let Some(receipt) = app.receipt.clone() else {
        return;
    };
    let tone = if receipt.warnings.is_empty() {
        Tone::Ok
    } else {
        Tone::Warn
    };
    let title = format!(
        "{}{}",
        if receipt.warnings.is_empty() {
            m.done
        } else {
            m.partial
        },
        app.receipt_action
            .map(|action| format!(" · {}", action.label(m)))
            .unwrap_or_default()
    );
    let body = headline(frame, area, tone, &title);
    let mut lines = Vec::new();
    if receipt.enrolled {
        lines.push(plain(m.enrollment_done));
        lines.push(Line::raw(""));
    }
    if receipt.left {
        lines.push(plain(m.leave_done));
        lines.push(Line::raw(""));
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
            "snapshot" => m.refresh,
            "connection" => m.connection,
            "sync" => m.description,
            "delivery" => m.invite,
            "hub_notice" => m.hub_title,
            "enrollment" => m.stage_enrolled,
            _ => m.facts,
        };
        let value = match (key.as_str(), value.as_str()) {
            (_, "saved") => m.saved,
            (_, "queued" | "reconnect_requested") => m.queued,
            (_, "waiting_for_node") => m.waiting_node,
            (_, "waiting_for_connection") => m.waiting_connection,
            (_, "not_installed") => m.not_installed,
            (_, "refreshed") | ("enrollment", "removed") | ("result", _) => m.done,
            ("hub_notice", "revoked") => m.revoked,
            ("service", _) if !receipt.warnings.is_empty() => m.partial,
            ("service", _) => match app.receipt_action {
                Some(MenuAction::Start | MenuAction::Restart) => m.running,
                Some(MenuAction::Stop | MenuAction::Uninstall | MenuAction::Leave) => m.not_running,
                Some(MenuAction::Install | MenuAction::Create | MenuAction::Join) => m.installed,
                _ => m.done,
            },
            _ => value,
        };
        lines.push(pair(label, value));
    }
    for warning in &receipt.warnings {
        lines.push(judged(Tone::Warn.symbol(), Tone::Warn.color(), warning));
    }
    if let Some(invite) = &receipt.invite {
        lines.push(Line::raw(""));
        lines.push(muted(m.invite_help));
        lines.push(Line::raw(""));
        // Laid out here rather than left to the wrapper, so the block that holds a string the
        // user has to select whole comes out as a rectangle instead of a ragged edge.
        let ground = Style::default().fg(theme::fg()).bg(theme::surface());
        let inner = usize::from(body.width).saturating_sub(2).max(8);
        for row in split_width(invite, inner) {
            lines.push(Line::from(Span::styled(
                format!(" {} ", padded(&row, inner)),
                ground,
            )));
        }
        lines.push(Line::raw(""));
    }
    scroll(frame, app, body, None, lines);
}

pub(super) fn action_name(action: &Action, m: &i18n::WorldMessages) -> &'static str {
    MenuAction::from_action(action).label(m)
}

fn preview(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    let Some(action) = &app.preview else {
        return vec![plain(m.empty)];
    };
    let this_machine = app
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.config.get())
        .map(|config| config.name.as_str())
        .unwrap_or(m.unknown);
    match action {
        Action::Init {
            hub,
            name,
            network,
            install_service,
            ..
        } => vec![
            pair(m.name, name),
            pair(m.hub_address, hub),
            pair(m.network, network.as_str()),
            pair(m.auto_service, yes(*install_service, m)),
            Line::raw(""),
            plain(m.enroll_warning),
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
                pair(m.name, name),
                pair(m.hub_address, hub),
                pair(m.network, network.as_str()),
                pair(m.auto_service, yes(*install_service, m)),
                Line::raw(""),
                plain(m.enroll_warning),
            ]
        }
        Action::Invite { name, hours, hubs } => vec![
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
            Line::raw(""),
            plain(m.hours_help),
        ],
        Action::Revoke { name } => vec![pair(m.name, name), Line::raw(""), warn(m.revoke_warning)],
        Action::Leave => vec![
            pair(m.this_machine, this_machine),
            Line::raw(""),
            warn(m.leave_warning),
        ],
        Action::Grant {
            grant,
            expected_revision,
        } => vec![
            pair(m.principal, &grant.principal),
            pair(m.targets, grant.nodes.join(", ")),
            pair(m.verbs, grant.verbs.join(", ")),
            pair(m.expires, grant.expires.as_deref().unwrap_or(m.none)),
            pair(m.grant_revision, expected_revision),
            Line::raw(""),
            warn(m.grant_warning),
        ],
        Action::DeleteGrant {
            id,
            expected_revision,
        } => {
            let mut lines = Vec::new();
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
            lines.extend([
                pair("ID", id),
                pair(m.grant_revision, expected_revision),
                Line::raw(""),
                warn(m.delete_grant_warning),
            ]);
            lines
        }
        Action::Service { action } => vec![
            pair(m.this_machine, this_machine),
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
            Line::raw(""),
            plain(m.service_warning),
        ],
        Action::HubRevoke { data, name } => vec![
            pair(m.name, name),
            pair(m.directory, data.to_string_lossy()),
            Line::raw(""),
            warn(m.hub_revoke_warning),
        ],
        _ => vec![plain(m.continue_action)],
    }
}

fn node_error(app: &WorldApp, m: &i18n::WorldMessages, check_nodes: bool) -> Option<Lines> {
    let snapshot = app.snapshot.as_ref()?;
    match &snapshot.node {
        ReadState::Ready(node) => match &node.nodes {
            ReadState::Failed(error) if check_nodes => Some(failure_lines(error, m)),
            _ => None,
        },
        ReadState::Failed(error) => Some(failure_lines(error, m)),
        ReadState::Missing => Some(vec![muted(m.unavailable_body)]),
    }
}

/// An empty value says "nothing here", which a blank line to the right of a label does not.
fn or_none(value: String, m: &i18n::WorldMessages) -> String {
    if value.trim().is_empty() {
        m.none.to_string()
    } else {
        value
    }
}

fn node_details(node: &NodeView, m: &i18n::WorldMessages) -> Lines {
    let presence = Presence::from_state(&node.state);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} {}", presence.symbol(), presence.label(m)),
                Style::default()
                    .fg(presence.color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  ·  {} · {}{}",
                    os_label(&node.os),
                    node.arch,
                    if node.is_self {
                        format!("  ·  {}", m.this_machine)
                    } else {
                        String::new()
                    }
                ),
                Style::default().fg(theme::muted()),
            ),
        ]),
        pair(
            m.last_seen,
            format!(
                "{}{}",
                relative(&node.last_seen, m),
                node.hub_rtt_ms
                    .filter(|_| presence == Presence::Online)
                    .map(|rtt| format!("  ·  {rtt} ms"))
                    .unwrap_or_default()
            ),
        ),
        Line::raw(""),
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
            lines.push(pair(m.ai_sessions, m.unknown));
            lines.push(pair(m.data_time, relative(&activity.observed_at, m)));
        }
        None => lines.push(pair(m.ai_sessions, m.unknown)),
    }
    lines.extend([
        Line::raw(""),
        section(m.note),
        pair(m.effective_note, node.effective_note().unwrap_or(m.none)),
        pair(
            m.private_note,
            node.private_note.as_deref().unwrap_or(m.none),
        ),
        pair(
            m.description,
            node.inventory
                .as_ref()
                .map(|inventory| inventory.description.as_str())
                .filter(|text| !text.is_empty())
                .unwrap_or(m.none),
        ),
        muted(m.note_help),
        Line::raw(""),
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
                    format!("{}  ", m.disks),
                    Style::default().fg(theme::muted()),
                ),
                Span::styled(
                    format!("{}  ", safe(&disk.mount)),
                    Style::default().fg(theme::fg()),
                ),
                bar(used, 12, color),
                Span::styled(
                    format!("  {} / {} GiB", disk.free_gb, disk.total_gb),
                    Style::default().fg(theme::muted()),
                ),
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
        let shell = [inv.shell.kind.as_str(), inv.shell.path.as_str()]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ");
        lines.extend([
            pair(m.shell, or_none(shell, m)),
            pair(m.capabilities, or_none(inv.capabilities.join(", "), m)),
            pair(m.tags, or_none(node.tags.join(", "), m)),
            pair(m.addresses, or_none(inv.addresses.join(", "), m)),
            pair(m.version, &node.version),
            pair(m.inventory_time, relative(&inv.collected_at, m)),
        ]);
        if let Some(error) = &inv.shell.error {
            lines.push(warn(error));
        }
    } else {
        lines.push(pair(m.hardware, m.unknown));
        lines.push(pair(m.tags, or_none(node.tags.join(", "), m)));
        lines.push(pair(m.version, &node.version));
    }
    lines.extend([
        Line::raw(""),
        section(m.connection),
        pair(m.network, node.network.as_deref().unwrap_or(m.unknown)),
        pair(m.tls, node.tls.as_deref().unwrap_or(m.unknown)),
    ]);
    lines
}

fn detail_title(app: &WorldApp, m: &i18n::WorldMessages) -> String {
    match app.page {
        Page::Overview | Page::Machines => app
            .focused_node()
            .map(|node| safe(&node.name))
            .unwrap_or_else(|| m.machines.to_string()),
        Page::Grants => app
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.node.get())
            .and_then(|node| node.grants.grants.get(app.selected[Page::Grants.index()]))
            .map(|entry| safe(&entry.grant.principal))
            .unwrap_or_else(|| m.grants.to_string()),
        Page::Events => app
            .events
            .get(app.selected[Page::Events.index()])
            .map(|event| model::event_label(&event.kind, m).0.to_string())
            .unwrap_or_else(|| m.events.to_string()),
        Page::Hub => app
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.hub.get())
            .and_then(|hub| hub.members.get(app.selected[Page::Hub.index()]))
            .map(|member| safe(&member.name))
            .unwrap_or_else(|| m.hub.to_string()),
        Page::Connection => m.connection.to_string(),
        Page::Service => m.service.to_string(),
    }
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
            lines.push(muted(format!("  {remedy}")));
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
        Line::raw(""),
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
        Line::raw(""),
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
        Line::raw(""),
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
                    Line::raw(""),
                    muted(m.grants_help),
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
                    Line::raw(""),
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
            lines.push(Line::raw(""));
            lines.extend(service_lines(app, m));
            lines
        }
    }
}

fn help_lines(app: &WorldApp, m: &i18n::WorldMessages) -> Lines {
    if app.help_from == Screen::Receipt
        && let Some(receipt) = &app.receipt
    {
        let mut lines = receipt
            .facts
            .iter()
            .map(|(key, value)| pair(key, value))
            .collect::<Vec<_>>();
        lines.extend(receipt.warnings.iter().map(warn));
        return lines;
    }
    if app.help_from == Screen::Form
        && let Some(form) = &app.form
    {
        let mut lines = Vec::new();
        if let Some(index) = form.focused_field() {
            let field = &form.fields[index];
            lines.push(section(field.kind.label(m, &form.kind)));
            lines.push(plain(field.kind.help(m, &form.kind)));
            lines.push(Line::raw(""));
        }
        if let Some(error) = &form.error {
            lines.push(warn(form.error_text(m)));
            lines.extend(classified_failure_lines(error, m));
            lines.push(Line::raw(""));
        }
        lines.push(muted(m.form_keys));
        lines.push(muted(m.edit_keys));
        return lines;
    }
    if app.page != Page::Hub && !app.enrolled() {
        return vec![
            section(m.welcome),
            plain(m.welcome_body),
            Line::raw(""),
            plain(m.setup_help),
            Line::raw(""),
            plain(m.join_help),
            Line::raw(""),
            muted(m.shortcuts),
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
            lines.push(Line::raw(""));
            lines.extend(detail(app, m));
            lines
        }
        Page::Machines => detail(app, m),
        Page::Grants => {
            let mut lines = vec![plain(m.grants_help), muted(m.grants_default), Line::raw("")];
            lines.extend(detail(app, m));
            lines
        }
        Page::Connection => connection_lines(app, m),
        Page::Events => {
            let mut lines = vec![
                plain(m.cursor_help),
                pair(m.cursor, app.event_cursor),
                pair(m.events, app.event_latest),
                Line::raw(""),
            ];
            if let Some(error) = &app.event_error {
                lines.extend(classified_failure_lines(error, m));
            }
            lines.extend(detail(app, m));
            lines
        }
        Page::Service => {
            let mut lines = vec![plain(m.stage_help), Line::raw("")];
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
    lines.push(Line::raw(""));
    lines.push(section(m.actions));
    let recommended = app.recommended();
    for action in app.actions() {
        let mut spans = vec![Span::styled(
            action.label(m).to_string(),
            Style::default().fg(theme::fg()),
        )];
        if let Some(key) = action.hotkey() {
            spans.push(Span::styled(
                format!("  {key}"),
                Style::default().fg(theme::muted()),
            ));
        }
        if recommended == Some(action) {
            spans.push(Span::styled(
                format!("  · {}", m.recommended),
                Style::default().fg(theme::success()),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::raw(""));
    lines.push(muted(m.shortcuts));
    lines.push(muted(m.page_keys));
    lines
}

/// What went wrong and what to do about it. The caller's own title already names the state, so
/// this does not restate it — repeating the classification above the detail reads as two
/// separate problems, and "check this field" is nonsense over a file that could not be read.
fn failure_lines(error: &Failure, m: &i18n::WorldMessages) -> Lines {
    let mut lines = vec![plain(&error.detail)];
    if error.kind == FailureKind::Permission {
        lines.push(muted(m.permission_help));
    }
    if error.uncertain {
        lines.push(warn(m.uncertain));
    }
    lines
}

/// The same, led by the kind of failure; for screens whose title names the action rather than
/// the state. Only the two kinds that change what the user should do next are named.
fn classified_failure_lines(error: &Failure, m: &i18n::WorldMessages) -> Lines {
    let mut lines = failure_lines(error, m);
    match error.kind {
        FailureKind::Permission => lines.insert(0, warn(m.permission)),
        FailureKind::Unavailable => lines.insert(0, warn(m.unavailable)),
        _ => {}
    }
    lines
}
fn yes(value: bool, m: &i18n::WorldMessages) -> &'static str {
    if value { m.yes } else { m.no }
}
