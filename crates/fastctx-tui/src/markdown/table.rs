//! Content-sized columns and labeled records for tables that cannot fit side by side.

use pulldown_cmark::Alignment;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::text;

type Cell = Vec<Span<'static>>;
type Row = Vec<Cell>;

// A URL or identifier may wrap after 24 cells instead of imposing its full length on a column.
// Three such columns and their two-cell gaps fit the 76-cell page on an 84-cell screen.
const WORD_WIDTH: usize = 24;
const GAP: usize = 2;

pub(super) fn render(
    rows: &[Row],
    alignments: &[Alignment],
    width: usize,
    body: Style,
    faint: Style,
) -> Vec<Line<'static>> {
    let mut natural = vec![1; alignments.len()];
    let mut minimum = vec![1; alignments.len()];
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            let content = content(cell);
            let longest_line = content.lines().map(text::width).max().unwrap_or(0);
            let grapheme = content.graphemes(true).map(text::width).max().unwrap_or(0);
            natural[column] = natural[column].max(longest_line);
            minimum[column] = minimum[column].max(text::unbreakable_width(&content).min(WORD_WIDTH).max(grapheme));
        }
    }
    let gaps = GAP * (natural.len() - 1);
    if minimum.iter().sum::<usize>() + gaps > width {
        return records(rows, width, body, faint);
    }
    let widths = column_widths(natural, &minimum, width - gaps);
    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        lines.extend(row_lines(row, &widths, alignments, body));
        if index == 0 {
            let mut spans = Vec::new();
            for (column, width) in widths.iter().enumerate() {
                if column > 0 {
                    spans.push(Span::styled(" ".repeat(GAP), faint));
                }
                spans.push(Span::styled("─".repeat(*width), faint));
            }
            lines.push(Line::from(spans));
        }
    }
    lines
}

fn content(cell: &[Span<'_>]) -> String {
    text::Layout::default().expand_tabs(&cell.iter().map(|span| span.content.as_ref()).collect::<String>())
}

fn column_widths(natural: Vec<usize>, minimum: &[usize], available: usize) -> Vec<usize> {
    if natural.iter().sum::<usize>() <= available {
        return natural;
    }
    let at = |column: usize, level: usize| natural[column].min(minimum[column].max(level));
    // Lowering a common ceiling shrinks the widest columns first. Binary search avoids a
    // pass for every cell of a long source line; the minimum widths are never crossed.
    let mut low = 0;
    let mut high = *natural.iter().max().expect("a table has columns");
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if (0..natural.len()).map(|column| at(column, middle)).sum::<usize>() <= available {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let mut widths: Vec<_> = (0..natural.len()).map(|column| at(column, low)).collect();
    let mut remaining = available - widths.iter().sum::<usize>();
    for (column, width) in widths.iter_mut().enumerate() {
        if remaining > 0 && *width == low && *width < natural[column] {
            *width += 1;
            remaining -= 1;
        }
    }
    widths
}

fn row_lines(row: &[Cell], widths: &[usize], alignments: &[Alignment], body: Style) -> Vec<Line<'static>> {
    let wrapped: Vec<_> = row.iter().zip(widths).map(|(cell, width)| text::wrap(cell, *width)).collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(0);
    (0..height)
        .map(|line| {
            let mut spans = Vec::new();
            for (column, cells) in wrapped.iter().enumerate() {
                let cell = cells.get(line).cloned().unwrap_or_default();
                let free = widths[column].saturating_sub(text::line_width(&cell));
                let left = match alignments[column] {
                    Alignment::Center => free / 2,
                    Alignment::Right => free,
                    _ => 0,
                };
                spans.push(Span::styled(" ".repeat(left), body));
                spans.extend(cell.spans);
                if column + 1 < widths.len() {
                    spans.push(Span::styled(" ".repeat(free - left + GAP), body));
                }
            }
            Line::from(spans)
        })
        .collect()
}

fn records(rows: &[Row], width: usize, body: Style, faint: Style) -> Vec<Line<'static>> {
    let headers: Vec<_> =
        rows[0].iter().map(|cell| content(cell).split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    let longest = headers.iter().map(|header| text::width(header)).max().unwrap_or(0);
    let label_grapheme = headers.iter().flat_map(|header| header.graphemes(true)).map(text::width).max().unwrap_or(0);
    let value_grapheme = rows[1..]
        .iter()
        .flatten()
        .map(|cell| content(cell).graphemes(true).map(text::width).max().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let gap = if longest == 0 { 0 } else { GAP };
    let stacked = label_grapheme + gap + value_grapheme > width;
    // Give long labels at most half the row unless their widest grapheme needs more room.
    // Reserve enough space for a value grapheme before assigning the label column.
    let label_width =
        longest.min(width.saturating_sub(gap) / 2).max(label_grapheme).min(width.saturating_sub(gap + value_grapheme));
    let value_width = width.saturating_sub(label_width + gap);
    let mut lines = Vec::new();
    if rows.len() == 1 {
        // Streaming input can contain a complete header and no records yet.
        for header in &headers {
            lines.extend(text::wrap_plain(header, faint, width));
        }
        return lines;
    }
    for (index, row) in rows[1..].iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        for (header, value) in headers.iter().zip(row) {
            let labels = if label_width > 0 { text::wrap_plain(header, faint, label_width) } else { Vec::new() };
            let values = text::wrap(value, value_width);
            // A glyph can fit while its punctuation-bound group cannot.
            // Use the existing stacked layout when either column overflows.
            if stacked
                || labels.iter().any(|line| text::line_width(line) > label_width)
                || values.iter().any(|line| text::line_width(line) > value_width)
            {
                // If even a label glyph, the gap and a value glyph cannot share the row,
                // let the field break after its label. Neither label nor value is truncated.
                if !header.is_empty() {
                    lines.extend(text::wrap_plain(header, faint, width));
                }
                lines.extend(text::wrap(value, width));
                continue;
            }
            for line in 0..labels.len().max(values.len()) {
                let label = labels.get(line).cloned().unwrap_or_default();
                let padding = label_width - text::line_width(&label) + gap;
                let mut spans = label.spans;
                spans.push(Span::styled(" ".repeat(padding), body));
                if let Some(value) = values.get(line) {
                    spans.extend(value.spans.iter().cloned());
                }
                lines.push(Line::from(spans));
            }
        }
    }
    lines
}
