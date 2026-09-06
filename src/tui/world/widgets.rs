//! The drawing vocabulary every World page shares: text lines, filled cards, title rules,
//! stat tiles, tables, key bars and bounded text layout.
//!
//! Two rules keep the pages coherent. Structure comes from filled grounds and thin rules, not
//! from nested borders, and every state is carried by a glyph before it is carried by a colour,
//! because the sixteen-colour palette folds `muted` onto `fg` and monochrome folds everything.

use super::{WorldApp, model::Tone};
use crate::tui::theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Widget,
};
use unicode_segmentation::UnicodeSegmentation;

pub(super) type Lines = Vec<Line<'static>>;

/// Display width in terminal cells; CJK and emoji count double, combining marks count zero.
pub(super) fn width(text: &str) -> usize {
    Span::raw(text).width()
}

/// Escapes control characters and invisible direction marks so data can never draw the screen.
pub(super) fn safe(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch == '\n' {
                " \\n ".into()
            } else if ch.is_control()
                || matches!(
                    ch,
                    '\u{061c}'
                        | '\u{200b}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{2028}'..='\u{202e}'
                        | '\u{2060}'..='\u{2069}'
                        | '\u{feff}'
                )
            {
                format!("\\u{:04x}", ch as u32)
            } else {
                ch.to_string()
            }
        })
        .collect()
}

pub(super) fn plain(value: impl ToString) -> Line<'static> {
    Line::from(Span::styled(
        safe(&value.to_string()),
        Style::default().fg(theme::fg()),
    ))
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
pub(super) fn strong(value: impl ToString) -> Line<'static> {
    Line::from(Span::styled(
        safe(&value.to_string()),
        Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
    ))
}
/// `label  value`: the label recedes, the value reads.
pub(super) fn pair(label: &str, value: impl ToString) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}  "), Style::default().fg(theme::muted())),
        Span::styled(safe(&value.to_string()), Style::default().fg(theme::fg())),
    ])
}
/// A group heading inside a fact list.
pub(super) fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
    ))
}
/// One judgement line: a coloured symbol, then the sentence.
pub(super) fn judged(symbol: &str, color: Color, text: impl ToString) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{symbol} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(safe(&text.to_string()), Style::default().fg(theme::fg())),
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

pub(super) fn selection_style() -> Style {
    let style = Style::default()
        .fg(theme::accent())
        .bg(theme::highlight())
        .add_modifier(Modifier::BOLD);
    if theme::monochrome() {
        style.add_modifier(Modifier::REVERSED)
    } else {
        style
    }
}

/// A pressable label. The focused face is filled with its colour; in monochrome it is
/// reversed, which is the same thing said with the only means left.
pub(super) fn button(label: &str, focused: bool, color: Color) -> Span<'static> {
    let style = if focused {
        let style = Style::default()
            .fg(theme::bg())
            .bg(color)
            .add_modifier(Modifier::BOLD);
        if theme::monochrome() {
            style.add_modifier(Modifier::REVERSED)
        } else {
            style
        }
    } else {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    };
    Span::styled(format!("[ {label} ]"), style)
}

/// A key cap followed by what it does; the caps sit on `surface` so they read as keys.
pub(super) fn key_cap(key: &str, label: &str) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!(" {key} "),
        Style::default()
            .fg(theme::accent())
            .bg(theme::surface())
            .add_modifier(Modifier::BOLD),
    )];
    if !label.is_empty() {
        spans.push(Span::styled(
            format!(" {label}"),
            Style::default().fg(theme::muted()),
        ));
    }
    spans
}

/// The footer line: every key still meaningful on this screen, labels dropped before keys when
/// the terminal is too narrow, and never more than fits.
pub(super) fn key_bar(keys: &[(&str, &str)], width: u16) -> Line<'static> {
    let gap = "  ";
    let measure = |with_labels: bool, count: usize| {
        keys.iter()
            .take(count)
            .map(|(key, label)| {
                width_of(key)
                    + 2
                    + if with_labels && !label.is_empty() {
                        width_of(label) + 1
                    } else {
                        0
                    }
            })
            .sum::<usize>()
            + gap.len() * count.saturating_sub(1)
    };
    let available = usize::from(width);
    let (with_labels, count) = if measure(true, keys.len()) <= available {
        (true, keys.len())
    } else if measure(false, keys.len()) <= available {
        (false, keys.len())
    } else {
        let mut count = keys.len();
        while count > 0 && measure(false, count) > available {
            count -= 1;
        }
        (false, count)
    };
    let mut spans = Vec::new();
    for (index, (key, label)) in keys.iter().take(count).enumerate() {
        if index > 0 {
            spans.push(Span::raw(gap));
        }
        spans.extend(key_cap(key, if with_labels { label } else { "" }));
    }
    Line::from(spans).alignment(Alignment::Center)
}

fn width_of(text: &str) -> usize {
    width(text)
}

/// `title ───────── right`: a heading that also separates, without spending a border.
pub(super) fn title_rule(frame: &mut Frame<'_>, area: Rect, title: &str, right: Option<&str>) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let right = right.map(safe).unwrap_or_default();
    let title = clipped(
        &safe(title),
        usize::from(area.width).saturating_sub(width(&right) + 4),
    );
    let used = width(&title)
        + if right.is_empty() {
            0
        } else {
            width(&right) + 1
        }
        + 1;
    let rule = usize::from(area.width).saturating_sub(used);
    let mut spans = vec![
        Span::styled(
            title,
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", "─".repeat(rule)),
            Style::default().fg(theme::border()),
        ),
    ];
    if !right.is_empty() {
        spans.push(Span::styled(
            format!(" {right}"),
            Style::default().fg(theme::muted()),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { height: 1, ..area },
    );
}

pub(super) fn fill(buf: &mut Buffer, area: Rect, color: Color) {
    if theme::monochrome() {
        return;
    }
    buf.set_style(area, Style::default().bg(color));
}

/// Flattens whatever is already drawn in `area` to one recessive shade, so the popup laid over
/// it reads as the live layer while the page beneath still says where the user is.
pub(super) fn dim(buf: &mut Buffer, area: Rect) {
    let style = Style::reset().fg(theme::border()).bg(theme::bg());
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(style);
            }
        }
    }
}

/// A filled card, or an outlined one where a fill would vanish into the page. The focused
/// card is one shade lighter and carries a bar on its left edge, which is what a monochrome
/// terminal is left with. Returns the text area inside the card.
pub(super) fn card(buf: &mut Buffer, area: Rect, selected: bool) -> Rect {
    if area.width < 4 || area.height == 0 {
        return area;
    }
    if theme::flat() {
        let block = Block::default()
            .borders(Borders::ALL)
            // A flat palette has no lighter ground to lift the focused card with, and
            // monochrome has no colour left either, so the frame itself carries the focus.
            .border_type(if selected {
                BorderType::Thick
            } else {
                BorderType::Rounded
            })
            .border_style(Style::default().fg(if selected {
                theme::accent()
            } else {
                theme::border()
            }));
        let inner = block.inner(area);
        block.render(area, buf);
        return Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };
    }
    fill(
        buf,
        area,
        if selected {
            theme::highlight()
        } else {
            theme::surface()
        },
    );
    if selected {
        for row in area.y..area.bottom() {
            if let Some(cell) = buf.cell_mut((area.x, row)) {
                cell.set_symbol("▌").set_fg(theme::accent());
            }
        }
    }
    Rect {
        x: area.x + 2,
        width: area.width.saturating_sub(3),
        ..area
    }
}

pub(super) struct Tile {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) color: Color,
}

impl Tile {
    pub(super) fn new(label: &str, value: impl ToString, color: Color) -> Self {
        Self {
            label: label.to_string(),
            value: safe(&value.to_string()),
            color,
        }
    }
}

/// A row of label-over-value figures with no frames; as many as fit, in order.
pub(super) fn stat_tiles(frame: &mut Frame<'_>, area: Rect, tiles: &[Tile]) {
    if area.height < 2 || area.width == 0 {
        return;
    }
    let mut x = area.x;
    for tile in tiles {
        let tile_width = width(&tile.label).max(width(&tile.value)) as u16;
        if x + tile_width > area.right() {
            break;
        }
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    tile.label.clone(),
                    Style::default().fg(theme::muted()),
                )),
                Line::from(Span::styled(
                    tile.value.clone(),
                    Style::default().fg(tile.color).add_modifier(Modifier::BOLD),
                )),
            ]),
            Rect::new(x, area.y, tile_width, 2),
        );
        x += tile_width + 4;
    }
}

/// A centred, cleared, outlined box for menus and confirmations. Returns the inner area.
pub(super) fn popup(
    frame: &mut Frame<'_>,
    area: Rect,
    width: u16,
    height: u16,
    title: &str,
    color: Color,
) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 3,
        width,
        height,
    );
    // A cleared gutter around the card: without it the page underneath ends flush against the
    // border, and an outlined palette leaves half a card poking out beside the dialog.
    let gutter = Rect {
        x: popup.x.saturating_sub(1),
        y: popup.y.saturating_sub(1),
        width: (popup.width + 2).min(area.right() - popup.x.saturating_sub(1)),
        height: (popup.height + 2).min(area.bottom() - popup.y.saturating_sub(1)),
    };
    frame.render_widget(Clear, gutter);
    frame.render_widget(
        Block::default().style(Style::default().fg(theme::fg()).bg(theme::bg())),
        gutter,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(
            format!(
                " {} ",
                clipped(&safe(title), usize::from(width).saturating_sub(4))
            ),
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme::surface()).fg(theme::fg()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    }
}

/// A vertically centred notice for loading, empty and error states.
pub(super) fn notice(frame: &mut Frame<'_>, area: Rect, tone: Tone, title: &str, body: Lines) {
    let width = usize::from(area.width).saturating_sub(4).min(72);
    let mut lines = vec![judged(tone.symbol(), tone.color(), title)];
    if let Some(first) = lines.first_mut() {
        for span in &mut first.spans[1..] {
            span.style = span.style.add_modifier(Modifier::BOLD);
        }
    }
    if !body.is_empty() {
        lines.push(Line::raw(""));
        lines.extend(body);
    }
    let mut lines = wrapped(lines, width);
    // Say that something was left out rather than stopping mid-sentence, which reads as a fault.
    if lines.len() > usize::from(area.height) && area.height > 0 {
        lines.truncate(usize::from(area.height));
        if let Some(last) = lines.last_mut() {
            *last = muted("…");
        }
    }
    let height = (lines.len() as u16).min(area.height);
    let top = area.y + area.height.saturating_sub(height) / 3;
    let left = area.x + (area.width.saturating_sub(width as u16)) / 2;
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(left, top, width as u16, height),
    );
}

/// Scrollable text with an optional title rule; the rule's right side shows the position.
pub(super) fn scroll(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    area: Rect,
    title: Option<&str>,
    lines: Lines,
) {
    let has_title = title.is_some() && area.height >= 3;
    let body = if has_title {
        Rect {
            y: area.y + 2,
            height: area.height - 2,
            ..area
        }
    } else {
        area
    };
    let lines = wrapped(lines, body.width as usize);
    app.viewport_rows = body.height.max(1) as usize;
    app.max_scroll = lines.len().saturating_sub(body.height as usize);
    app.scroll = app.scroll.min(app.max_scroll);
    if has_title {
        let position =
            (app.max_scroll > 0).then(|| format!("{}/{} ↑↓", app.scroll + 1, app.max_scroll + 1));
        title_rule(frame, area, title.unwrap_or(""), position.as_deref());
    }
    frame.render_widget(
        Paragraph::new(lines).scroll((app.scroll.min(u16::MAX as usize) as u16, 0)),
        body,
    );
}

/// Exact column widths for `table`: the fixed ones as given, the last taking what remains after
/// the selection marker and the gaps, so no column is squeezed behind the caller's back.
/// Returns the constraints and the width of that last column.
pub(super) fn columns(
    area_width: u16,
    fixed: &[usize],
) -> (Vec<ratatui::layout::Constraint>, usize) {
    let used = fixed.iter().sum::<usize>() + 2 * fixed.len() + 2;
    let last = usize::from(area_width).saturating_sub(used).max(4);
    let mut constraints = fixed
        .iter()
        .map(|width| ratatui::layout::Constraint::Length(*width as u16))
        .collect::<Vec<_>>();
    constraints.push(ratatui::layout::Constraint::Length(last as u16));
    (constraints, last)
}

/// A dense table with a muted header, one row per object and a full-width focused row.
/// The caller sizes columns; the table keeps the focused row in view.
pub(super) fn table(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    area: Rect,
    header: &[&str],
    rows: Vec<Row<'static>>,
    widths: &[ratatui::layout::Constraint],
    selected: usize,
) {
    if area.height == 0 {
        return;
    }
    let header_row = Row::new(
        header
            .iter()
            .map(|title| {
                Cell::from(Span::styled(
                    title.to_string(),
                    Style::default().fg(theme::muted()),
                ))
            })
            .collect::<Vec<_>>(),
    )
    .bottom_margin(0);
    app.viewport_rows = area.height.saturating_sub(1).max(1) as usize;
    let mut state = TableState::default().with_selected((!rows.is_empty()).then_some(selected));
    frame.render_stateful_widget(
        Table::new(rows, widths.to_vec())
            .header(header_row)
            .column_spacing(2)
            .row_highlight_style(selection_style())
            .highlight_symbol(Span::styled("▌ ", Style::default().fg(theme::accent()))),
        area,
        &mut state,
    );
}

pub(super) fn wrapped(lines: Lines, width: usize) -> Lines {
    if width == 0 {
        return Vec::new();
    }
    let mut output = Vec::new();
    for line in lines {
        let alignment = line.alignment;
        let mut current = Vec::new();
        let mut used = 0;
        let mut continued = false;
        for span in line.spans {
            let mut words: Vec<String> = Vec::new();
            for word in span.content.split_word_bounds() {
                if word
                    .chars()
                    .all(|ch| ".,;:!?%)]}。，、；：！？％）】》」』".contains(ch))
                    && let Some(previous) = words
                        .last_mut()
                        .filter(|text| !text.chars().all(char::is_whitespace))
                {
                    previous.push_str(word);
                } else {
                    words.push(word.to_string());
                }
            }
            for word in words {
                let size = self::width(&word);
                if used > 0 && used + size > width {
                    let mut row = Line::from(std::mem::take(&mut current));
                    row.alignment = alignment;
                    output.push(row);
                    used = 0;
                    continued = true;
                }
                if continued && used == 0 && word.chars().all(char::is_whitespace) {
                    continue;
                }
                if size <= width {
                    current.push(Span::styled(word, span.style));
                    used += size;
                } else {
                    for grapheme in word.graphemes(true) {
                        let size = self::width(grapheme);
                        if used + size > width && !current.is_empty() {
                            let mut row = Line::from(std::mem::take(&mut current));
                            row.alignment = alignment;
                            output.push(row);
                            used = 0;
                            continued = true;
                        }
                        current.push(Span::styled(
                            if size > width { "…" } else { grapheme }.to_string(),
                            span.style,
                        ));
                        used += size.min(width);
                    }
                }
            }
        }
        let mut row = Line::from(current);
        row.alignment = alignment;
        output.push(row);
    }
    output
}

/// Cuts a string to `width` cells with an ellipsis instead of wrapping; for table-like rows.
pub(super) fn clipped(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if self::width(text) <= width {
        return text.to_string();
    }
    let mut used = 0;
    let mut output = String::new();
    for grapheme in text.graphemes(true) {
        let size = self::width(grapheme);
        if used + size + 1 > width {
            break;
        }
        output.push_str(grapheme);
        used += size;
    }
    output.push('…');
    output
}

/// Keeps at most `limit` rows and marks the cut, so text that continues past the fold says so.
pub(super) fn elided(mut lines: Lines, limit: usize, width: usize) -> Lines {
    if lines.len() <= limit || limit == 0 {
        return lines;
    }
    lines.truncate(limit);
    if let Some(last) = lines.last_mut() {
        let text = last
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        let head = split_width(&text, width.saturating_sub(2))
            .into_iter()
            .next()
            .unwrap_or_default();
        *last = muted(format!("{head} …"));
    }
    lines
}

/// Cuts a string into rows of exactly `width` cells, breaking wherever the row fills up.
/// For opaque strings such as an invite, which have no words to wrap on.
pub(super) fn split_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let size = self::width(grapheme);
        if used + size > width {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        row.push_str(grapheme);
        used += size;
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// Pads with spaces to exactly `width` cells, clipping first when needed.
pub(super) fn padded(text: &str, width: usize) -> String {
    let text = clipped(text, width);
    let missing = width.saturating_sub(self::width(&text));
    format!("{text}{}", " ".repeat(missing))
}
