//! How every page is laid out: a column of text in the middle of the screen,
//! like a page of a letter, with the page's name at the top and the keys at
//! the bottom. Things are told apart by weight, brightness and space, never
//! by lines; the selected item sits on a ground a shade off, with a mark in
//! the gutter.

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::design::{Look, Role};
use crate::text;

/// The widest a column of text grows: about as wide as a line is still
/// comfortable to read, with room for a status on the right.
const COLUMN: u16 = 104;

/// The mark beside the selected item.
pub const MARK: &str = "▍";

/// Where the text of a page goes, inside `area`.
pub fn column(area: Rect) -> Rect {
    let margin = if area.width >= 60 { 4 } else { 2 };
    let width = area.width.saturating_sub(margin * 2).min(COLUMN);
    let x = area.x + (area.width - width) / 2;
    Rect { x, y: area.y, width, height: area.height }
}

/// One row of a page: text on the left, a quieter fact on the right, and
/// the item it belongs to when it can be selected.
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub left: Line<'static>,
    pub right: Line<'static>,
    pub item: Option<usize>,
    /// A hairline runs from the left side to the right: a section's title.
    pub rule: bool,
}

impl Row {
    pub fn blank() -> Self {
        Self::default()
    }

    pub fn new(left: impl Into<Line<'static>>) -> Self {
        Self { left: left.into(), ..Self::default() }
    }

    pub fn right(mut self, right: impl Into<Line<'static>>) -> Self {
        self.right = right.into();
        self
    }

    pub fn item(mut self, item: usize) -> Self {
        self.item = Some(item);
        self
    }
}

/// Rows of the same item, with `indent` cells before each.
pub fn indented(lines: Vec<Line<'static>>, indent: usize, item: Option<usize>) -> Vec<Row> {
    lines
        .into_iter()
        .map(|mut line| {
            if indent > 0 {
                line.spans.insert(0, Span::raw(" ".repeat(indent)));
            }
            Row { left: line, right: Line::default(), item, rule: false }
        })
        .collect()
}

/// Fills `area` with the ground.
pub fn ground(buffer: &mut Buffer, area: Rect, look: &Look) {
    buffer.set_style(area, look.ground());
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buffer[(x, y)].set_symbol(" ").set_diff_option(CellDiffOption::None);
        }
    }
}

/// Paints rows from `top` into `area` (a column), the selected item raised.
pub fn rows(buffer: &mut Buffer, area: Rect, rows: &[Row], top: usize, selected: Option<usize>, look: &Look) {
    for (offset, row) in rows.iter().skip(top).take(area.height as usize).enumerate() {
        let y = area.y + offset as u16;
        let raised = row.item.is_some() && row.item == selected;
        if raised {
            // The band reaches into the gutter, so the mark sits on it.
            if look.color(Role::Raised).is_some() {
                let band = Rect { x: area.x.saturating_sub(2), y, width: area.width + 3, height: 1 };
                buffer.set_style(band.intersection(buffer.area), look.raised(Role::Text));
            }
            // The mark runs the item's whole height, like its band.
            text::Layout::default().write_line(
                buffer,
                Rect::new(area.x.saturating_sub(2), y, text::width(MARK) as u16, 1),
                &Line::styled(MARK, look.raised(Role::Accent)),
            );
        }
        if row.rule {
            let right = text::line_width(&row.right);
            let room = if right > 0 { (area.width as usize).saturating_sub(right + 2) } else { area.width as usize };
            let run = room.saturating_sub(text::line_width(&row.left) + 1);
            let mut left = row.left.clone();
            if run > 0 {
                left.spans.push(Span::raw(" "));
                left.spans.push(Span::styled("─".repeat(run), look.text(Role::Rule)));
            }
            line(buffer, area.x, y, area.width, &left, &row.right, raised, look);
        } else {
            line(buffer, area.x, y, area.width, &row.left, &row.right, raised, look);
        }
    }
}

/// Paints one line, its right side flush with the column's edge; the left
/// side gives way when both do not fit.
#[allow(clippy::too_many_arguments)]
pub fn line(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    left: &Line<'_>,
    right: &Line<'_>,
    raised: bool,
    look: &Look,
) {
    line_with_layout(buffer, x, y, width, left, right, raised, look, &text::Layout::default());
}

/// Paints with the same terminal measurements used to wrap the content.
#[allow(clippy::too_many_arguments)]
pub fn line_with_layout(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    left: &Line<'_>,
    right: &Line<'_>,
    raised: bool,
    look: &Look,
    layout: &text::Layout<'_>,
) {
    let width = width as usize;
    let right_width = layout.line_width(right);
    let (right_width, room) =
        if right_width > 0 && right_width + 2 < width { (right_width, width - right_width - 2) } else { (0, width) };
    let mut left = layout.fit_line(left, room);
    let lift = |style: Style| if raised { on_raised(style, look) } else { style };
    left.style = lift(left.style);
    for span in &mut left.spans {
        span.style = lift(span.style);
    }
    layout.write_line(buffer, Rect::new(x, y, room as u16, 1), &left);
    if right_width > 0 {
        let mut right = layout.fit_line(right, right_width);
        right.style = lift(right.style);
        for span in &mut right.spans {
            span.style = lift(span.style);
        }
        let at = x + (width - right_width) as u16;
        layout.write_line(buffer, Rect::new(at, y, right_width as u16, 1), &right);
    }
}

/// The same style on the raised ground.
fn on_raised(style: Style, look: &Look) -> Style {
    match look.color(Role::Raised) {
        Some(raised) => style.bg(raised),
        None => style.add_modifier(Modifier::BOLD),
    }
}

/// A line cut to `max` cells, ending in `…` when anything was cut.
pub fn fit_line(line: &Line<'_>, max: usize) -> Line<'static> {
    text::Layout::default().fit_line(line, max)
}

/// Keeps the item spanning rows `first..=last` in view in `height` rows.
pub fn keep_in_view(top: usize, first: usize, last: usize, height: usize) -> usize {
    if first < top {
        first
    } else if last >= top + height {
        (last + 1).saturating_sub(height).min(first)
    } else {
        top
    }
}

/// The rows an item spans.
pub fn item_rows(rows: &[Row], item: usize) -> Option<(usize, usize)> {
    let first = rows.iter().position(|row| row.item == Some(item))?;
    let last = rows.iter().rposition(|row| row.item == Some(item))?;
    Some((first, last))
}

/// The page's name at its top: where it is in the World, and on the right
/// what is true of it now.
pub fn header(buffer: &mut Buffer, area: Rect, look: &Look, crumbs: &[String], right: &Line<'_>) {
    let mut spans = Vec::new();
    for (index, crumb) in crumbs.iter().enumerate() {
        let last = index + 1 == crumbs.len();
        if index > 0 {
            spans.push(Span::styled("  ›  ", look.text(Role::Faint)));
        }
        let style = if last { look.text(Role::Text).add_modifier(Modifier::BOLD) } else { look.text(Role::Muted) };
        spans.push(Span::styled(crumb.clone(), style));
    }
    line(buffer, area.x, area.y, area.width, &Line::from(spans), right, false, look);
}

/// The keys of a page at its bottom: each key a little brighter than what
/// it does.
pub fn footer(buffer: &mut Buffer, area: Rect, look: &Look, keys: &[(&str, &str)]) {
    let mut keys = keys.to_vec();
    let width = |keys: &[(&str, &str)]| -> usize {
        keys.iter().map(|(key, does)| text::width(key) + 1 + text::width(does)).sum::<usize>()
            + 4 * keys.len().saturating_sub(1)
    };
    while keys.len() > 2 && width(&keys) > area.width as usize {
        keys.remove(keys.len() - 2);
    }
    let mut spans = Vec::new();
    for (index, (key, does)) in keys.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("    ", look.text(Role::Faint)));
        }
        spans.push(Span::styled(key.to_string(), look.text(Role::Muted)));
        spans.push(Span::styled(format!(" {does}"), look.text(Role::Faint)));
    }
    line(buffer, area.x, area.y, area.width, &Line::from(spans), &Line::default(), false, look);
}

/// A section's name: quiet, but where the eye finds it.
/// A section's title, with a hairline to its right.
pub fn section(title: &str, look: &Look) -> Row {
    Row { rule: true, ..Row::new(Span::styled(title.to_string(), look.text(Role::Muted).add_modifier(Modifier::BOLD))) }
}

/// Starts a section under what is there: two blank rows above its title
/// and one under it, so the title sits nearer what it names than what came
/// before, and a section reads as one however its last item ended.
pub fn open_section(rows: &mut Vec<Row>, title: Row) {
    while rows.last().is_some_and(|row| text::line_width(&row.left) == 0 && text::line_width(&row.right) == 0) {
        rows.pop();
    }
    if !rows.is_empty() {
        rows.extend([Row::blank(), Row::blank()]);
    }
    rows.extend([title, Row::blank()]);
}

/// The parts of a page: its name, its body and its keys.
pub struct Frame {
    pub header: Rect,
    pub body: Rect,
    pub note: Rect,
    pub footer: Rect,
}

/// Splits a screen into a page: a blank line, the name, two blank lines,
/// the body, a line for a note, the keys, and a blank line under them.
pub fn frame(area: Rect) -> Frame {
    let column = column(area);
    let at = |offset: u16| Rect { y: column.y + offset, height: 1, ..column };
    let bottom = column.height.saturating_sub(1);
    Frame {
        header: at(1.min(bottom)),
        body: Rect { y: column.y + 4, height: column.height.saturating_sub(8), ..column },
        note: at(bottom.saturating_sub(3)),
        footer: at(bottom.saturating_sub(1)),
    }
}
