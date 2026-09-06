//! Shared terminal panels, bounded text layout and data-safe display cells for World views.

use super::WorldApp;
use crate::tui::theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};
use unicode_segmentation::UnicodeSegmentation;

pub(super) type Lines = Vec<Line<'static>>;

pub(super) fn choice_style(selected: bool) -> Style {
    Style::default()
        .fg(theme::accent())
        .add_modifier(if selected {
            Modifier::REVERSED | Modifier::BOLD
        } else {
            Modifier::empty()
        })
}
pub(super) fn plain(value: impl ToString) -> Line<'static> {
    Line::from(safe(&value.to_string()))
}
pub(super) fn muted(value: impl ToString) -> Line<'static> {
    Line::from(Span::styled(
        safe(&value.to_string()),
        Style::default().fg(theme::muted()),
    ))
}
pub(super) fn warn(value: impl ToString) -> Line<'static> {
    Line::from(Span::styled(
        safe(&value.to_string()),
        Style::default().fg(theme::warning()),
    ))
}
pub(super) fn pair(label: &str, value: impl ToString) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(theme::muted())),
        Span::raw(safe(&value.to_string())),
    ])
}
/// A group heading inside a fact list; the symbol makes it a heading without colour.
pub(super) fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("▸ {title}"),
        Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
    ))
}
/// One judgement line: a coloured symbol, then the sentence.
pub(super) fn judged(symbol: &str, color: Color, text: impl ToString) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{symbol} "), Style::default().fg(color)),
        Span::raw(safe(&text.to_string())),
    ])
}
/// A horizontal proportion bar of block elements; `fraction` is the filled part.
pub(super) fn bar(fraction: f32, width: usize, color: Color) -> Span<'static> {
    let filled = ((fraction.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    Span::styled(
        format!("{}{}", "█".repeat(filled), "░".repeat(width - filled)),
        Style::default().fg(color),
    )
}
pub(super) fn safe(text: &str) -> String {
    text.chars().map(|ch| if ch=='\n'{" ↵ ".into()}else if ch.is_control()||matches!(ch,'\u{061c}'|'\u{200b}' | '\u{200e}' | '\u{200f}'|'\u{2028}'..='\u{202e}'|'\u{2060}'..='\u{2069}'|'\u{feff}'){format!("\\u{:04x}",ch as u32)}else{ch.to_string()}).collect()
}

pub(super) fn card(frame: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}
pub(super) fn scroll(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    area: Rect,
    title: &str,
    lines: Lines,
) {
    let inner = card(frame, area, title);
    let lines = wrapped(lines, inner.width as usize);
    app.viewport_rows = inner.height.max(1) as usize;
    app.max_scroll = lines.len().saturating_sub(inner.height as usize);
    app.scroll = app.scroll.min(app.max_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((app.scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
    if app.max_scroll > 0 && area.width > 12 && area.height > 1 {
        frame.render_widget(
            Paragraph::new(format!(" {}/{} ", app.scroll + 1, app.max_scroll + 1))
                .style(Style::default().fg(theme::muted())),
            Rect::new(area.right().saturating_sub(12), area.y, 11, 1),
        );
    }
}
pub(super) fn list(frame: &mut Frame<'_>, area: Rect, title: &str, items: Lines, selected: usize) {
    let inner = card(frame, area, title);
    let mut state = ListState::default().with_selected((!items.is_empty()).then_some(selected));
    let items = items
        .into_iter()
        .map(|item| ListItem::new(wrapped(vec![item], inner.width.saturating_sub(2) as usize)))
        .collect::<Vec<_>>();
    frame.render_stateful_widget(
        List::new(items).highlight_symbol("› ").highlight_style(
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::REVERSED),
        ),
        inner,
        &mut state,
    );
}
pub(super) fn intro_list(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    intro: Lines,
    items: Lines,
    selected: usize,
) {
    let lines = wrapped(intro, area.width.saturating_sub(2) as usize);
    let reserve = if area.height >= 14 { 7 } else { 3 };
    let intro_height = (lines.len() as u16).min(area.height.saturating_sub(reserve));
    let parts = Layout::default()
        .constraints([Constraint::Length(intro_height), Constraint::Min(3)])
        .split(area);
    frame.render_widget(Paragraph::new(lines), parts[0]);
    list(frame, parts[1], title, items, selected);
}
pub(super) fn wrapped(lines: Lines, width: usize) -> Lines {
    if width == 0 {
        return Vec::new();
    }
    let mut output = Vec::new();
    for line in lines {
        let mut current = Vec::new();
        let mut used = 0;
        for span in line.spans {
            for grapheme in span.content.graphemes(true) {
                let size = Span::raw(grapheme).width();
                if used + size > width && !current.is_empty() {
                    output.push(Line::from(std::mem::take(&mut current)));
                    used = 0;
                }
                current.push(Span::styled(grapheme.to_string(), span.style));
                used += size;
            }
        }
        output.push(Line::from(current));
    }
    output
}
/// Cuts a line to `width` cells with an ellipsis instead of wrapping; for table-like rows.
pub(super) fn clipped(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let total = Span::raw(text).width();
    if total <= width {
        return text.to_string();
    }
    let mut used = 0;
    let mut output = String::new();
    for grapheme in text.graphemes(true) {
        let size = Span::raw(grapheme).width();
        if used + size + 1 > width {
            break;
        }
        output.push_str(grapheme);
        used += size;
    }
    output.push('…');
    output
}
