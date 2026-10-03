//! Page framing shared by every World screen: one header line naming where the user is, the
//! page tabs, and one footer line holding only the keys that mean something right now.

use super::forms::{FieldKind, FormKind};
use super::model::service_facts;
use super::widgets::{clipped, key_bar, safe, width};
use super::{Page, Screen, WorldApp, i18n};
use crate::tui::theme;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

pub(super) struct Regions {
    pub(super) body: Rect,
    pub(super) footer: Rect,
    pub(super) compact: bool,
}

/// Below this the page keeps only what fits: a one-line header, no tabs, single-column bodies.
pub(super) fn is_compact(area: Rect) -> bool {
    area.width < 60 || area.height < 16
}

pub(super) fn render(
    frame: &mut Frame<'_>,
    app: &WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
) -> Regions {
    frame.render_widget(
        Block::default().style(Style::default().fg(theme::fg()).bg(theme::bg())),
        area,
    );
    let compact = is_compact(area);
    let tabs = !compact
        && matches!(app.screen, Screen::Page | Screen::Detail | Screen::Actions)
        && app.shows_tabs();
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(!compact)),
        Constraint::Length(u16::from(tabs)),
        Constraint::Length(u16::from(!compact)),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);
    header(frame, app, m, parts[0], compact);
    if !compact {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "─".repeat(usize::from(parts[1].width)),
                Style::default().fg(theme::border()),
            )),
            parts[1],
        );
    }
    if tabs {
        render_tabs(frame, app, m, parts[2]);
    }
    let margin = if compact { 1 } else { 2 };
    let body = Rect {
        x: parts[4].x + margin,
        width: parts[4].width.saturating_sub(margin * 2),
        ..parts[4]
    };
    Regions {
        body,
        footer: parts[5],
        compact,
    }
}

/// What the header names after "World": the object the screen is about.
fn context(app: &WorldApp, m: &i18n::WorldMessages) -> String {
    match app.screen {
        Screen::Form => app
            .form
            .as_ref()
            .map(|form| form.title(m).to_string())
            .unwrap_or_default(),
        Screen::Confirm => app
            .preview
            .as_ref()
            .map(|action| super::view::action_name(action, m).to_string())
            .unwrap_or_else(|| m.preview.to_string()),
        Screen::Discard => m.discard.to_string(),
        Screen::Busy => app
            .active
            .as_ref()
            .map(|action| super::view::action_name(action, m).to_string())
            .unwrap_or_else(|| m.busy.to_string()),
        Screen::Receipt => if app
            .receipt
            .as_ref()
            .is_some_and(|receipt| !receipt.warnings.is_empty())
        {
            m.partial
        } else {
            m.done
        }
        .to_string(),
        Screen::Failed => m.failed.to_string(),
        Screen::Help => m.facts.to_string(),
        Screen::Talk => super::towns_view::chat_context(app, m),
        Screen::Page | Screen::Detail | Screen::Actions => {
            if app.page == Page::Hub {
                m.hub.to_string()
            } else if let Some(config) = app
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.config.get())
            {
                safe(&config.name)
            } else {
                m.overview.to_string()
            }
        }
    }
}

fn header(
    frame: &mut Frame<'_>,
    app: &WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
    compact: bool,
) {
    let mut context = context(app, m);
    if compact && matches!(app.screen, Screen::Page | Screen::Detail | Screen::Actions) {
        let page = format!(
            "{} {}/{}",
            app.page.title(m),
            app.page.index() + 1,
            Page::ALL.len()
        );
        context = match app
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.config.get())
        {
            Some(config) if app.shows_tabs() => format!("{} · {page}", safe(&config.name)),
            _ if app.shows_tabs() => page,
            _ => app.page.title(m).to_string(),
        };
    }
    if let Some(worker) = app.worker.as_ref().filter(|worker| !worker.read_only) {
        context = format!(
            "{context} · {} {}s",
            m.busy,
            worker.started.elapsed().as_secs()
        );
    }
    let mut left = vec![
        Span::styled(
            " FastCtx",
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::border())),
        Span::styled(
            "World",
            Style::default()
                .fg(theme::fg())
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if compact {
        left = vec![Span::styled(
            " World",
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        )];
    }
    let mut right = Vec::new();
    if app.shows_tabs()
        && matches!(app.screen, Screen::Page | Screen::Detail | Screen::Actions)
        && let Some(snapshot) = &app.snapshot
    {
        // A refresh that failed outranks whatever the last snapshot said: this badge answers
        // "is what I am looking at still true", and a stale page must not claim a live link.
        let (symbol, label, color) = if app.load_error.is_some() {
            ("!", m.stale, theme::warning())
        } else {
            let facts = service_facts(snapshot);
            match facts.running {
                Some(false) => ("○", m.not_running, theme::warning()),
                None => ("!", m.unknown, theme::warning()),
                Some(true) => match facts.link {
                    Some(link) => (link.symbol(), link.label(m), link.color()),
                    None => ("!", m.unknown, theme::warning()),
                },
            }
        };
        right.push(Span::styled(
            format!("{symbol} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        right.push(Span::styled(
            label.to_string(),
            Style::default().fg(theme::fg()),
        ));
    }
    if !compact && area.width >= 90 {
        right.push(Span::styled(
            format!("   v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme::muted()),
        ));
    }
    let separator = if compact { " › " } else { "  ›  " };
    let left_width = left.iter().map(Span::width).sum::<usize>() + width(separator);
    let mut right_width = right.iter().map(Span::width).sum::<usize>() + 3;
    // On a narrow terminal the place the user is in outranks the status badge, but the symbol
    // goes last: it is the only thing that says whether anything is wrong.
    let needed = |right_width: usize| left_width + width(&context) + right_width;
    if compact && needed(right_width) > usize::from(area.width) {
        right.truncate(1);
        right_width = right.iter().map(Span::width).sum::<usize>() + 3;
        if needed(right_width) > usize::from(area.width) {
            right.clear();
            right_width = 0;
        }
    }
    let room = usize::from(area.width)
        .saturating_sub(left_width + right_width)
        .max(4);
    left.push(Span::styled(
        separator,
        Style::default().fg(theme::border()),
    ));
    left.push(Span::styled(
        clipped(&context, room),
        Style::default().fg(theme::fg()),
    ));
    frame.render_widget(Paragraph::new(Line::from(left)), area);
    if !right.is_empty() {
        right.push(Span::raw(" "));
        frame.render_widget(
            Paragraph::new(Line::from(right)).alignment(Alignment::Right),
            area,
        );
    }
}

fn render_tabs(frame: &mut Frame<'_>, app: &WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let titles = Page::ALL
        .iter()
        .enumerate()
        .map(|(index, page)| (index + 1, page.title(m)))
        .collect::<Vec<_>>();
    let needed = |numbered: bool| {
        titles
            .iter()
            .map(|(number, title)| {
                width(title)
                    + 2
                    + if numbered {
                        number.to_string().len() + 1
                    } else {
                        0
                    }
            })
            .sum::<usize>()
            + 2 * titles.len()
    };
    let mut spans = vec![Span::raw(" ")];
    let numbered = needed(true) <= usize::from(area.width);
    if numbered || needed(false) <= usize::from(area.width) {
        for (index, (number, title)) in titles.iter().enumerate() {
            let active = index == app.page.index();
            let number = if numbered {
                format!("{number} ")
            } else {
                String::new()
            };
            if active {
                let style = Style::default()
                    .fg(theme::accent())
                    .bg(theme::surface())
                    .add_modifier(Modifier::BOLD);
                spans.push(Span::styled(
                    format!(" {number}{title} "),
                    if theme::flat() {
                        style.add_modifier(Modifier::UNDERLINED)
                    } else {
                        style
                    },
                ));
            } else {
                spans.push(Span::styled(
                    format!(" {number}"),
                    Style::default().fg(theme::border()),
                ));
                spans.push(Span::styled(
                    format!("{title} "),
                    Style::default().fg(theme::muted()),
                ));
            }
            spans.push(Span::raw("  "));
        }
    } else {
        spans.push(Span::styled(
            format!(" {} ", app.page.title(m)),
            Style::default()
                .fg(theme::accent())
                .bg(theme::surface())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("  {}/{}  ←→", app.page.index() + 1, Page::ALL.len()),
            Style::default().fg(theme::muted()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub(super) fn footer(frame: &mut Frame<'_>, app: &WorldApp, m: &i18n::WorldMessages, area: Rect) {
    let keys = keys(app, m);
    frame.render_widget(Paragraph::new(key_bar(&keys, area.width)), area);
}

fn keys<'a>(app: &WorldApp, m: &'a i18n::WorldMessages) -> Vec<(&'a str, &'a str)> {
    let has_invite = app
        .receipt
        .as_ref()
        .is_some_and(|receipt| receipt.invite.is_some());
    match app.screen {
        Screen::Page => {
            let mut keys = Vec::new();
            if app.count() > 0 || app.max_scroll > 0 {
                keys.push(("↑↓", m.key_select));
            }
            keys.push((
                "Enter",
                if app.mutation_running() {
                    m.busy
                } else {
                    app.primary_action()
                        .map(|action| action.label(m))
                        .unwrap_or(m.key_open)
                },
            ));
            if !app.mutation_running() {
                keys.push(("a", m.actions));
            }
            if app.shows_tabs() {
                keys.push(("←→", m.key_pages));
            }
            keys.push(("Esc", m.back));
            keys
        }
        Screen::Actions => vec![
            ("↑↓", m.key_select),
            ("Enter", m.continue_action),
            ("Esc", m.back),
        ],
        Screen::Detail => vec![("↑↓", m.key_scroll), ("a", m.actions), ("Esc", m.back)],
        Screen::Form => {
            let Some(form) = app.form.as_ref() else {
                return vec![("Esc", m.back)];
            };
            let mut keys = vec![("Tab", m.key_next)];
            match form.focus() {
                super::forms::Focus::Field(index) => {
                    let field = &form.fields[index];
                    if field.kind.is_choice() {
                        keys.push(("←→", m.key_toggle));
                    }
                    if field.input.secret {
                        keys.push(("F2", m.key_reveal));
                    }
                    let multiline = match field.kind {
                        FieldKind::Text => {
                            matches!(form.kind, FormKind::Description | FormKind::PrivateNote(_))
                        }
                        FieldKind::What | FieldKind::Places => true,
                        _ => false,
                    };
                    if multiline {
                        keys.push(("Ctrl-J", m.key_newline));
                    }
                }
                super::forms::Focus::Advanced => keys.push(("Enter", m.key_toggle)),
                super::forms::Focus::Submit => {}
            }
            keys.push(("Ctrl-S", form.submit_label(m)));
            keys.push(("Esc", m.back));
            keys
        }
        Screen::Confirm | Screen::Discard => {
            let mut keys = vec![("←→", m.key_select)];
            if app.max_scroll > 0 {
                keys.push(("↑↓", m.key_scroll));
            }
            keys.push(("Enter", m.continue_action));
            keys.push(("Esc", m.cancel));
            keys
        }
        Screen::Busy => vec![("Esc", m.back)],
        Screen::Receipt => {
            let mut keys = Vec::new();
            if has_invite {
                keys.push(("e", m.export));
            }
            if app.max_scroll > 0 {
                keys.push(("↑↓", m.key_scroll));
            }
            keys.push(("Enter", m.continue_action));
            keys
        }
        Screen::Failed => {
            let mut keys = Vec::new();
            if app.max_scroll > 0 {
                keys.push(("↑↓", m.key_scroll));
            }
            keys.push(("Enter", m.back));
            keys
        }
        Screen::Help => vec![("↑↓", m.key_scroll), ("Esc", m.back)],
        Screen::Talk => {
            let Some(chat) = app.towns.chat.as_ref() else {
                return vec![("Esc", m.back)];
            };
            if chat.client.is_none() {
                return vec![
                    ("Tab", m.key_next),
                    ("←→", m.key_select),
                    ("Enter", m.open_resident),
                    ("Esc", m.back),
                ];
            }
            let mut keys = vec![
                ("Enter", m.send),
                ("Ctrl-J", m.key_newline),
                ("PgUp", m.key_scroll),
            ];
            if chat
                .client
                .as_ref()
                .is_some_and(|client| client.transcript.busy)
            {
                keys.push(("Ctrl-X", m.stop_turn));
            }
            keys.push(("Esc", m.back));
            keys
        }
    }
}
