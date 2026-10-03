//! Drawing the Towns page and the chat screen. Everything shown comes from the records and the
//! node's events; nothing here is a judgement of what a resident is thinking.

use super::i18n::{WorldMessages, fill};
use super::model::{Tone, relative};
use super::talk::{Closed, Entry, ToolState};
use super::towns::{Census, CensusError, Chat, Field};
use super::widgets::{
    Lines, button, clipped, muted, notice, pair, plain, safe, section, table, table_window,
    title_rule, width, wrapped,
};
use super::{Page, WorldApp};
use crate::town::records::{About, Birth};
use crate::town::who::{JobFacts, ResidentFacts};
use crate::tui::theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row};

/// Recent actions the detail pane shows; the full twenty are one key away.
const PANE_ACTIONS: usize = 8;

pub(super) fn render_towns(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &WorldMessages,
    area: Rect,
    compact: bool,
) {
    let census = match &app.towns.census {
        None => return notice(frame, area, Tone::Info, m.loading, Vec::new()),
        Some(Err(CensusError::NotEnrolled)) => {
            return notice(
                frame,
                area,
                Tone::Info,
                m.towns_not_enrolled,
                vec![plain(m.towns_not_enrolled_body)],
            );
        }
        Some(Err(CensusError::Failed(error))) => {
            return notice(frame, area, Tone::Fail, m.error, vec![plain(error)]);
        }
        Some(Ok(census)) => census,
    };
    if census.residents.is_empty() {
        notice(
            frame,
            area,
            Tone::Info,
            m.no_residents,
            vec![plain(m.no_residents_body)],
        );
        return;
    }
    let wide = !compact && area.width >= 100;
    let panes = Layout::horizontal(if wide {
        vec![Constraint::Percentage(58), Constraint::Percentage(42)]
    } else {
        vec![Constraint::Percentage(100)]
    })
    .spacing(if wide { 3 } else { 0 })
    .split(area);
    let list = panes[0];
    let heading = fill(
        m.towns_heading,
        &[("n", &census.residents.len().to_string())],
    );
    title_rule(frame, list, &heading, Some(&reach(census, m)));
    let selected = app.selected[Page::Towns.index()];
    let table_area = Rect {
        y: list.y + 1,
        height: list.height.saturating_sub(1),
        ..list
    };
    let window = table_window(census.residents.len(), selected, table_area.height);
    let ids: Vec<String> = census
        .residents
        .iter()
        .map(|facts| safe(&facts.about.id))
        .collect();
    // A column is as wide as its content within a bound, and never narrower than its header.
    let id_width = ids
        .iter()
        .map(|id| width(id))
        .max()
        .unwrap_or(8)
        .clamp(8, 16)
        .max(width(m.col_resident));
    let state_width = census
        .residents
        .iter()
        .map(|facts| width(state_word(&facts.now.state, m)) + 2)
        .max()
        .unwrap_or(6)
        .max(width(m.col_state));
    let agents: Vec<String> = census
        .residents
        .iter()
        .map(|facts| agent_line(&facts.about))
        .collect();
    let agent_width = agents
        .iter()
        .map(|agent| width(agent))
        .max()
        .unwrap_or(6)
        .clamp(6, 24)
        .max(width(m.col_agent));
    let towns: Vec<String> = census
        .residents
        .iter()
        .map(|facts| town_label(facts.about.town.as_deref(), census, m))
        .collect();
    let town_width = towns
        .iter()
        .map(|town| width(town))
        .max()
        .unwrap_or(6)
        .clamp(6, 18)
        .max(width(m.col_town));
    let machines: Vec<String> = census
        .residents
        .iter()
        .map(|facts| safe(&facts.about.machine))
        .collect();
    let machine_width = machines
        .iter()
        .map(|machine| width(machine))
        .max()
        .unwrap_or(6)
        .clamp(6, 16)
        .max(width(m.col_machine));
    // Residents of one machine need no machine column; residents of several do, before their
    // town, which the detail pane names too.
    let with_machine = list.width >= 60 && machines.iter().any(|machine| machine != &machines[0]);
    let with_town = list.width >= if with_machine { 78 } else { 60 };
    let with_agent = list.width >= 110;
    let mut fixed = vec![state_width, id_width];
    if with_machine {
        fixed.push(machine_width);
    }
    if with_agent {
        fixed.push(agent_width);
    }
    if with_town {
        fixed.push(town_width);
    }
    let (constraints, doing_width) = super::widgets::columns(list.width, &fixed);
    let rows = census
        .residents
        .iter()
        .enumerate()
        .skip(window.start)
        .take(window.len())
        .map(|(index, facts)| {
            let (glyph, color) = state_mark(&facts.now.state);
            let mut cells = vec![
                Cell::from(Line::from(vec![
                    Span::styled(
                        format!("{glyph} "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        state_word(&facts.now.state, m).to_string(),
                        Style::default().fg(color),
                    ),
                ])),
                Cell::from(Span::styled(
                    clipped(&ids[index], id_width),
                    Style::default()
                        .fg(theme::fg())
                        .add_modifier(Modifier::BOLD),
                )),
            ];
            if with_machine {
                cells.push(Cell::from(Span::styled(
                    clipped(&machines[index], machine_width),
                    Style::default().fg(theme::muted()),
                )));
            }
            if with_agent {
                cells.push(Cell::from(Span::styled(
                    clipped(&agents[index], agent_width),
                    Style::default().fg(theme::muted()),
                )));
            }
            if with_town {
                cells.push(Cell::from(Span::styled(
                    clipped(&towns[index], town_width),
                    Style::default().fg(theme::fg()),
                )));
            }
            cells.push(Cell::from(Span::styled(
                clipped(&doing(facts, &census.running, m), doing_width),
                Style::default().fg(theme::muted()),
            )));
            Row::new(cells)
        })
        .collect::<Vec<_>>();
    let mut header = vec![m.col_state, m.col_resident];
    if with_machine {
        header.push(m.col_machine);
    }
    if with_agent {
        header.push(m.col_agent);
    }
    if with_town {
        header.push(m.col_town);
    }
    header.push(m.col_doing);
    let pane = census
        .residents
        .get(selected)
        .filter(|_| wide)
        .map(|facts| {
            let width = usize::from(panes[1].width);
            (
                safe(&facts.about.id),
                wrapped(resident_lines(facts, census, m, PANE_ACTIONS, width), width),
            )
        });
    table(
        frame,
        app,
        table_area,
        &header,
        rows,
        &constraints,
        selected.saturating_sub(window.start),
    );
    if let Some((title, lines)) = pane {
        let pane = panes[1];
        let body = Rect {
            y: pane.y + 2,
            height: pane.height.saturating_sub(2),
            ..pane
        };
        let more = lines.len() > usize::from(body.height);
        title_rule(
            frame,
            pane,
            &title,
            more.then(|| format!("F1 {}", m.facts)).as_deref(),
        );
        frame.render_widget(Paragraph::new(lines), body);
    }
}

/// The selected resident's full facts, for the detail screen.
pub(super) fn detail(app: &WorldApp, m: &WorldMessages) -> Lines {
    let Some(census) = app.towns.census() else {
        return vec![muted(m.loading)];
    };
    match census.residents.get(app.selected[Page::Towns.index()]) {
        Some(facts) => resident_lines(facts, census, m, usize::MAX, usize::MAX),
        None => vec![muted(m.no_residents)],
    }
}

pub(super) fn detail_title(app: &WorldApp, m: &WorldMessages) -> String {
    app.towns
        .residents()
        .get(app.selected[Page::Towns.index()])
        .map_or_else(|| m.towns.to_string(), |facts| safe(&facts.about.id))
}

/// Who a resident is, where, how it came, what it is doing, and what it did lately. The detail
/// read carries the actions and answers; until it arrives the pane shows what the list knows.
/// Each action keeps to one line of `width`, its result always in view.
fn resident_lines(
    facts: &ResidentFacts,
    census: &Census,
    m: &WorldMessages,
    actions: usize,
    width: usize,
) -> Lines {
    let about = &facts.about;
    let detail = census
        .detail
        .as_ref()
        .filter(|detail| detail.about.id == about.id)
        .unwrap_or(facts);
    let mut lines = vec![
        Line::from(Span::styled(
            agent_line(about),
            Style::default()
                .fg(theme::fg())
                .add_modifier(Modifier::BOLD),
        )),
        muted(format!(
            "{} · {} · {}",
            about.machine,
            surface_label(&about.surface, m),
            town_label(about.town.as_deref(), census, m)
        )),
        plain(came(about, m)),
        pair(m.here_for, span_since(&about.since)),
        Line::raw(""),
        section(m.section_now),
    ];
    let (glyph, color) = state_mark(&facts.now.state);
    lines.push(Line::from(vec![
        Span::styled(
            format!("{glyph} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{} · {}",
                state_word(&facts.now.state, m),
                relative(&facts.now.since, m)
            ),
            Style::default().fg(theme::fg()),
        ),
    ]));
    lines.push(muted(doing(facts, &census.running, m)));
    if facts.now.desk {
        lines.push(muted(m.using_desk));
    }
    if let Some(trouble) = &facts.now.trouble {
        let line = Line::from(vec![
            Span::styled(
                format!("{} ", Tone::Warn.symbol()),
                Style::default()
                    .fg(theme::warning())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} · {}", relative(&trouble.at, m), safe(&trouble.text)),
                Style::default().fg(theme::warning()),
            ),
        ]);
        lines.extend(wrapped(vec![line], width));
    }
    if !detail.actions.is_empty() {
        lines.push(Line::raw(""));
        lines.push(section(&fill(
            m.section_actions,
            &[
                ("n", &detail.actions.len().min(actions).to_string()),
                ("total", &detail.actions_total.to_string()),
            ],
        )));
        let skip = detail.actions.len().saturating_sub(actions);
        let shown = &detail.actions[skip..];
        let hand_width = shown
            .iter()
            .map(|action| super::widgets::width(&action.hand))
            .max()
            .unwrap_or(4);
        for action in shown {
            let at = action.at.get(11..16).unwrap_or(&action.at);
            let result = format!("  → {}", safe(&action.result));
            let room = width.saturating_sub(7 + hand_width + 1 + super::widgets::width(&result));
            lines.push(Line::from(vec![
                Span::styled(format!("{at}  "), Style::default().fg(theme::muted())),
                Span::styled(
                    super::widgets::padded(&action.hand, hand_width + 1),
                    Style::default().fg(theme::accent()),
                ),
                Span::styled(
                    clipped(&safe(&action.what), room.max(8)),
                    Style::default().fg(theme::fg()),
                ),
                Span::styled(result, Style::default().fg(theme::muted())),
            ]));
        }
    }
    let answers: Vec<_> = detail
        .messages
        .iter()
        .filter(|message| message.kind == "answered")
        .collect();
    if let Some(last) = answers.last() {
        lines.push(Line::raw(""));
        lines.push(section(&fill(
            m.section_answers,
            &[("n", &answers.len().to_string())],
        )));
        let to = if last.peer.is_empty() {
            m.nobody.to_string()
        } else {
            last.peer.clone()
        };
        lines.push(muted(fill(
            m.last_answer,
            &[("to", &to), ("ago", &relative(&last.at, m))],
        )));
        if let Some(text) = &last.text {
            lines.extend(text.lines().take(6).map(plain));
        }
    }
    lines
}

/// The chat screen: the conversation above, the line being typed below.
pub(super) fn render_chat(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &WorldMessages,
    area: Rect,
) {
    let census = app
        .towns
        .census
        .as_ref()
        .and_then(|census| census.as_ref().ok());
    let heading = match app.towns.chat_facts() {
        Some(facts) => format!("{} · {}", safe(&facts.about.id), agent_line(&facts.about)),
        None => app
            .towns
            .chat_resident()
            .map_or_else(|| m.new_resident.to_string(), safe),
    };
    // What the new-resident rows offer: the machines that answered, this one first, and the
    // towns that reach the chosen one.
    let chosen = app
        .towns
        .chat
        .as_ref()
        .and_then(|chat| chat.compose.as_ref())
        .map_or(0, |compose| compose.machine);
    let machines: Vec<String> = census
        .map(|census| census.machines.clone())
        .unwrap_or_default();
    let towns: Vec<(String, String)> = match (census, machines.get(chosen)) {
        (Some(census), Some(machine)) => census
            .towns_on(machine)
            .iter()
            .map(|town| (town.slug.clone(), town.name.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let Some(chat) = &mut app.towns.chat else {
        return;
    };
    let busy = chat
        .client
        .as_ref()
        .is_some_and(|client| client.transcript.busy);
    let state = chat
        .client
        .as_ref()
        .and_then(|client| client.transcript.state.clone());
    let right = match (busy, state.as_deref()) {
        (true, _) => Some(m.state_busy.to_string()),
        (false, Some(state)) => Some(state_word(state, m).to_string()),
        (false, None) => None,
    };
    title_rule(frame, area, &heading, right.as_deref());
    let parts = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    let conversation = parts[1];
    let lines = match &chat.client {
        None => compose_lines(chat, &machines, &towns, m, usize::from(conversation.width)),
        Some(_) => transcript_lines(chat, m, usize::from(conversation.width)),
    };
    let rows = usize::from(conversation.height);
    chat.page_rows = rows.saturating_sub(1).max(1);
    chat.max_back = lines.len().saturating_sub(rows);
    chat.back = chat.back.min(chat.max_back);
    let start = lines.len().saturating_sub(rows + chat.back);
    let shown: Lines = lines.into_iter().skip(start).take(rows).collect();
    frame.render_widget(Paragraph::new(shown), conversation);
    let hint = (chat.back > 0).then(|| fill(m.chat_scrolled, &[("n", &chat.back.to_string())]));
    rule(frame, parts[2], hint.as_deref());
    let placeholder = match chat
        .client
        .as_ref()
        .and_then(|client| client.resident.as_deref())
    {
        Some(resident) => fill(m.chat_placeholder, &[("resident", &safe(resident))]),
        None => m.compose_placeholder.to_string(),
    };
    frame.render_widget(
        Paragraph::new(input_line(chat, &placeholder, usize::from(parts[3].width))),
        parts[3],
    );
}

pub(super) fn chat_context(app: &WorldApp, m: &WorldMessages) -> String {
    app.towns
        .chat_resident()
        .map_or_else(|| m.new_resident.to_string(), safe)
}

fn compose_lines(
    chat: &Chat,
    machines: &[String],
    towns: &[(String, String)],
    m: &WorldMessages,
    columns: usize,
) -> Lines {
    let Some(compose) = &chat.compose else {
        return Vec::new();
    };
    // The labels take the width of the widest on screen, then two cells, so the choices line
    // up; the row the arrow keys change wears the accent and a marker.
    let label_width = [m.col_agent, m.col_machine, m.col_town]
        .iter()
        .map(|text| width(text))
        .max()
        .unwrap_or(0)
        + 2;
    let row = |field: Field, text: &str, choices: Vec<String>, chosen: usize| {
        let focused = compose.focus == field;
        let (marker, color) = if focused {
            ("› ", theme::accent())
        } else {
            ("  ", theme::muted())
        };
        let mut spans = vec![
            Span::styled(
                marker,
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{text}{}", " ".repeat(label_width - width(text))),
                Style::default().fg(color),
            ),
        ];
        for (index, choice) in choices.iter().enumerate() {
            spans.push(button(choice, index == chosen, theme::accent()));
            spans.push(Span::raw(" "));
        }
        if focused {
            spans.push(Span::styled(" ←/→", Style::default().fg(theme::border())));
        }
        Line::from(spans)
    };
    let agents = vec![m.agent_codex.to_string(), m.agent_claude.to_string()];
    let machines: Vec<String> = if machines.is_empty() {
        vec![m.this_machine.to_string()]
    } else {
        machines.iter().map(|machine| safe(machine)).collect()
    };
    let towns: Vec<String> = std::iter::once(m.outside.to_string())
        .chain(towns.iter().map(|(slug, name)| {
            if name == slug {
                safe(slug)
            } else {
                format!("{} · {}", safe(name), safe(slug))
            }
        }))
        .collect();
    wrapped(
        vec![
            row(Field::Agent, m.col_agent, agents, compose.agent),
            Line::raw(""),
            row(Field::Machine, m.col_machine, machines, compose.machine),
            Line::raw(""),
            row(Field::Town, m.col_town, towns, compose.town),
            Line::raw(""),
            muted(m.compose_hint),
        ],
        columns,
    )
}

fn transcript_lines(chat: &Chat, m: &WorldMessages, width: usize) -> Lines {
    let Some(client) = &chat.client else {
        return Vec::new();
    };
    let transcript = &client.transcript;
    let mut lines: Lines = Vec::new();
    if !transcript.live && transcript.closed.is_none() {
        lines.push(muted(m.chat_loading));
    }
    let mut previous_was_text = false;
    for entry in &transcript.entries {
        match entry {
            Entry::Words { from, text } => {
                if !lines.is_empty() {
                    lines.push(Line::raw(""));
                }
                let label = if from == "user" {
                    "› ".to_string()
                } else {
                    format!("‹{}› ", safe(from))
                };
                lines.extend(block(&label, text, width));
                lines.push(Line::raw(""));
                previous_was_text = false;
            }
            Entry::Message {
                text, streaming, ..
            } => {
                if previous_was_text {
                    lines.push(Line::raw(""));
                }
                let mut message: Lines = text.lines().map(plain).collect();
                if *streaming {
                    match message.last_mut() {
                        Some(last) => last
                            .spans
                            .push(Span::styled("▍", Style::default().fg(theme::accent()))),
                        None => message.push(Line::from(Span::styled(
                            "▍",
                            Style::default().fg(theme::accent()),
                        ))),
                    }
                }
                lines.extend(wrapped(message, width));
                previous_was_text = true;
            }
            Entry::Tool {
                name,
                summary,
                state,
                ..
            } => {
                let (glyph, color) = match state {
                    ToolState::Running => ("◦", theme::accent()),
                    ToolState::Done => (Tone::Ok.symbol(), Tone::Ok.color()),
                    ToolState::Failed => (Tone::Fail.symbol(), Tone::Fail.color()),
                };
                let name = safe(name);
                let room = width.saturating_sub(super::widgets::width(&name) + 4);
                let summary = summary.lines().next().unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{glyph} "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(name, Style::default().fg(theme::fg())),
                    Span::styled(
                        format!("  {}", clipped(&safe(summary), room)),
                        Style::default().fg(theme::muted()),
                    ),
                ]));
                previous_was_text = false;
            }
            Entry::TurnEnded { status, reason } => {
                let (label, color) = match status.as_str() {
                    "completed" => (m.turn_completed, theme::border()),
                    "interrupted" => (m.turn_interrupted, theme::warning()),
                    _ => (m.turn_failed, theme::danger()),
                };
                lines.push(Line::raw(""));
                lines.push(centered_rule(label, color, width));
                if let Some(reason) = reason {
                    lines.extend(wrapped(
                        vec![Line::from(Span::styled(
                            safe(reason),
                            Style::default().fg(color),
                        ))],
                        width,
                    ));
                }
                previous_was_text = false;
            }
            Entry::Health(text) => {
                let line = Line::from(vec![
                    Span::styled(
                        format!("{} ", Tone::Warn.symbol()),
                        Style::default()
                            .fg(theme::warning())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(safe(text), Style::default().fg(theme::warning())),
                ]);
                lines.extend(wrapped(vec![line], width));
                previous_was_text = false;
            }
            Entry::Refused(reason) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{} ", Tone::Warn.symbol()),
                        Style::default()
                            .fg(theme::warning())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        fill(m.chat_refused, &[("reason", &safe(reason))]),
                        Style::default().fg(theme::warning()),
                    ),
                ]));
                previous_was_text = false;
            }
        }
    }
    if transcript.busy {
        let working = match chat.busy_since {
            Some(since) => fill(
                m.chat_working_for,
                &[("elapsed", &span(since.elapsed().as_secs()))],
            ),
            None => m.chat_working.to_string(),
        };
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                "● ",
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(working, Style::default().fg(theme::muted())),
        ]));
    }
    if let Some(closed) = &transcript.closed {
        let text = match closed {
            Closed::NodeDown => m.chat_node_down.to_string(),
            Closed::Ended(None) => m.chat_ended.to_string(),
            Closed::Ended(Some(reason)) => format!("{} {}", m.chat_ended, safe(reason)),
            Closed::Failed(reason) => safe(reason),
        };
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", Tone::Warn.symbol()),
                Style::default()
                    .fg(theme::warning())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(text, Style::default().fg(theme::fg())),
        ]));
    }
    lines
}

/// Words said into the conversation: a filled block, so a turn reads from where it starts.
fn block(label: &str, text: &str, width: usize) -> Lines {
    let ground = Style::default().fg(theme::fg()).bg(theme::surface());
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let prefix = if index == 0 {
            label.to_string()
        } else {
            " ".repeat(super::widgets::width(label))
        };
        let source = Line::from(vec![
            Span::styled(
                prefix,
                ground.fg(theme::accent()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(safe(line), ground),
        ]);
        rows.extend(wrapped(vec![source], width.saturating_sub(1)));
    }
    rows.into_iter()
        .map(|mut row| {
            let used: usize = row.spans.iter().map(Span::width).sum();
            for span in &mut row.spans {
                span.style = span.style.bg(theme::surface());
            }
            row.spans.insert(0, Span::styled(" ", ground));
            row.spans.push(Span::styled(
                " ".repeat(width.saturating_sub(used + 1)),
                ground,
            ));
            row
        })
        .collect()
}

fn centered_rule(label: &str, color: Color, width: usize) -> Line<'static> {
    let label = format!(" {label} ");
    let side = width.saturating_sub(super::widgets::width(&label)) / 2;
    Line::from(vec![
        Span::styled("─".repeat(side), Style::default().fg(theme::border())),
        Span::styled(label, Style::default().fg(color)),
        Span::styled("─".repeat(side), Style::default().fg(theme::border())),
    ])
}

fn rule(frame: &mut Frame<'_>, area: Rect, right: Option<&str>) {
    let right = right.map(|text| format!(" {text}")).unwrap_or_default();
    let line = Line::from(vec![
        Span::styled(
            "─".repeat(usize::from(area.width).saturating_sub(super::widgets::width(&right))),
            Style::default().fg(theme::border()),
        ),
        Span::styled(right, Style::default().fg(theme::muted())),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn input_line(chat: &Chat, placeholder: &str, width: usize) -> Line<'static> {
    let ground = Style::default().fg(theme::fg()).bg(theme::surface());
    let mut spans = vec![Span::styled(
        "▌",
        Style::default().fg(theme::accent()).bg(theme::surface()),
    )];
    for cell in chat.input.cells(width.saturating_sub(2)) {
        let mut style = ground;
        if cell.selected || cell.cursor {
            style = style.add_modifier(Modifier::REVERSED);
        }
        spans.push(Span::styled(cell.text, style));
    }
    if chat.input.text().is_empty() {
        let room = width.saturating_sub(3);
        spans.push(Span::styled(
            format!(" {}", clipped(placeholder, room)),
            ground.fg(theme::muted()),
        ));
    }
    let used: usize = spans.iter().map(Span::width).sum();
    spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), ground));
    if theme::flat() {
        for span in &mut spans {
            span.style = span.style.add_modifier(Modifier::UNDERLINED);
        }
    }
    Line::from(spans)
}

/// A state's glyph and colour: the glyph first, so the state reads without colour.
fn state_mark(state: &str) -> (&'static str, Color) {
    match state {
        "busy" => ("●", theme::accent()),
        "waiting" => (Tone::Warn.symbol(), theme::warning()),
        "idle" => ("○", theme::fg()),
        "error" => (Tone::Fail.symbol(), theme::danger()),
        _ => (Tone::Info.symbol(), theme::muted()),
    }
}

fn state_word<'a>(state: &str, m: &'a WorldMessages) -> &'a str {
    match state {
        "busy" => m.state_busy,
        "idle" => m.state_idle,
        "waiting" => m.state_waiting,
        "asleep" => m.state_asleep,
        "interrupted" => m.state_interrupted,
        "error" => m.state_error,
        "left" => m.state_left,
        _ => m.unknown,
    }
}

/// Which machines the census covers: "desk · lab · hangzhou offline", or this machine alone
/// when its node is not running.
fn reach(census: &Census, m: &WorldMessages) -> String {
    if census.alone {
        return fill(m.node_alone, &[("machine", &safe(&census.machine))]);
    }
    let mut parts: Vec<String> = census
        .machines
        .iter()
        .map(|machine| safe(machine))
        .collect();
    for silent in &census.silent {
        let machine = safe(&silent.machine);
        parts.push(match &silent.why {
            crate::town::w::Silence::Offline(_) => {
                fill(m.machine_offline, &[("machine", &machine)])
            }
            crate::town::w::Silence::NoAnswer(_) => {
                fill(m.machine_silent, &[("machine", &machine)])
            }
        });
    }
    parts.join(" · ")
}

fn agent_line(about: &About) -> String {
    let agent = match about.agent.as_str() {
        "codex" | "codex-mcp-client" => "Codex",
        "claude-code" => "Claude Code",
        other => other,
    };
    match &about.model {
        Some(model) => format!("{agent} · {}", safe(model)),
        None => safe(agent),
    }
}

fn surface_label<'a>(surface: &'a str, m: &'a WorldMessages) -> &'a str {
    match surface {
        "vscode" | "claude-vscode" => "VS Code",
        "cli" => m.surface_terminal,
        "bg" => m.surface_background,
        "world" => m.surface_world,
        "app" | "claude-desktop" => m.surface_app,
        other => other,
    }
}

fn town_label(town: Option<&str>, census: &Census, m: &WorldMessages) -> String {
    match town {
        None => m.outside.to_string(),
        Some(slug) => census
            .towns
            .iter()
            .find(|town| town.slug == slug)
            .map_or_else(|| safe(slug), |town| safe(&town.name)),
    }
}

fn came(about: &About, m: &WorldMessages) -> String {
    match &about.birth {
        Birth::User => m.opened_by_user.to_string(),
        Birth::Colleague {
            requester, label, ..
        } => fill(
            m.asked_by,
            &[("requester", &safe(requester)), ("label", &safe(label))],
        ),
    }
}

/// What a resident is doing: the command it runs, else its last action and how long ago.
fn doing(facts: &ResidentFacts, running: &[JobFacts], m: &WorldMessages) -> String {
    if let Some(job) = running
        .iter()
        .filter(|job| job.resident == facts.about.id)
        .max_by(|left, right| left.started_at.cmp(&right.started_at))
    {
        let command = job
            .command
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
            .trim();
        return fill(
            m.running_for,
            &[
                ("machine", &safe(&job.machine)),
                ("elapsed", &span_since(&job.started_at)),
                ("command", &safe(command)),
            ],
        );
    }
    match (&facts.now.last_action, &facts.now.last_action_at) {
        (Some(action), Some(at)) => fill(
            m.did_ago,
            &[("action", &safe(action)), ("ago", &relative(at, m))],
        ),
        (Some(action), None) => safe(action),
        _ => m.no_actions_yet.to_string(),
    }
}

/// How long since an RFC 3339 moment, as `2h10m`.
fn span_since(at: &str) -> String {
    crate::world::parse_rfc3339(at)
        .map(|moment| {
            span(
                u64::try_from((time::OffsetDateTime::now_utc() - moment).whole_seconds())
                    .unwrap_or_default(),
            )
        })
        .unwrap_or_default()
}

fn span(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d{}h", seconds / 86_400, seconds % 86_400 / 3600),
    }
}
