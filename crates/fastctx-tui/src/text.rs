//! Text as the terminal shows it: every width is the width on screen, so
//! Chinese, Japanese and Korean take two cells, and lines break the way each
//! script breaks (`world-gui.md`, 中文按真实的显示宽度排).
//!
//! A line may break between any two CJK characters and at spaces between
//! words. Closing punctuation never starts a line and opening punctuation
//! never ends one (禁则): they stay with the character beside them. A word
//! wider than the whole line is cut where it has to be.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// `text` cut to `max` cells, ending in `…` when anything was cut.
pub fn fit(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let cells = width(grapheme);
        if used + cells > max - 1 {
            break;
        }
        out.push_str(grapheme);
        used += cells;
    }
    out.push('…');
    out
}

/// The first line of `text`, with `…` when more follows.
pub fn first_line(text: &str) -> String {
    let mut lines = text.trim().lines();
    let first = lines.next().unwrap_or_default().trim_end().to_string();
    if lines.next().is_some() { format!("{first} …") } else { first }
}

/// Characters that never start a line.
fn closes(grapheme: &str) -> bool {
    grapheme.chars().next().is_some_and(|c| "，。．、；：！？）」』】》〉〕｝’”…—～·,.;:!?)]}%".contains(c))
}

/// Characters that never end a line.
fn opens(grapheme: &str) -> bool {
    grapheme.chars().next().is_some_and(|c| "（「『【《〈〔｛‘“([{".contains(c))
}

/// Whether a line may break before and after this grapheme on its own,
/// as it may around each CJK character.
fn breaks_alone(grapheme: &str) -> bool {
    grapheme.chars().next().is_some_and(|c| {
        matches!(c,
            '\u{1100}'..='\u{115f}'
            | '\u{2e80}'..='\u{a4cf}'
            | '\u{ac00}'..='\u{d7a3}'
            | '\u{f900}'..='\u{faff}'
            | '\u{fe30}'..='\u{fe4f}'
            | '\u{ff00}'..='\u{ff60}'
            | '\u{ffe0}'..='\u{ffe6}'
            | '\u{20000}'..='\u{3fffd}'
        ) || UnicodeWidthStr::width(grapheme) > 1
    })
}

/// One grapheme with its style.
#[derive(Clone)]
struct Cell<'a> {
    text: &'a str,
    style: Style,
    width: usize,
}

/// Breaks styled text into lines of at most `width` cells. Newlines in the
/// text end lines; spaces where a line breaks are dropped.
pub fn wrap(spans: &[Span<'_>], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut paragraph: Vec<Cell<'_>> = Vec::new();
    for span in spans {
        for grapheme in span.content.graphemes(true) {
            if grapheme == "\n" || grapheme == "\r\n" {
                lines.extend(wrap_paragraph(&paragraph, width));
                paragraph.clear();
                continue;
            }
            let grapheme = if grapheme == "\t" { "    " } else { grapheme };
            paragraph.push(Cell { text: grapheme, style: span.style, width: UnicodeWidthStr::width(grapheme) });
        }
    }
    lines.extend(wrap_paragraph(&paragraph, width));
    lines
}

/// Plain text wrapped in one style.
pub fn wrap_plain(text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    wrap(&[Span::styled(text, style)], width)
}

/// The atoms of a paragraph: runs that stay on one line, each with the
/// spaces after it.
fn atoms(cells: &[Cell<'_>]) -> Vec<(usize, usize)> {
    let mut atoms = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < cells.len() {
        let cell = &cells[index];
        let space = cell.text.chars().all(char::is_whitespace);
        let next = cells.get(index + 1);
        let end_here = match next {
            None => true,
            Some(next) => {
                let next_space = next.text.chars().all(char::is_whitespace);
                if next_space {
                    false
                } else if space {
                    // Spaces belong to the atom before them; a word starts
                    // after them.
                    true
                } else if closes(next.text) || opens(cell.text) || next.width == 0 {
                    false
                } else {
                    breaks_alone(cell.text) || breaks_alone(next.text)
                }
            }
        };
        index += 1;
        if end_here {
            atoms.push((start, index));
            start = index;
        }
    }
    atoms
}

fn wrap_paragraph(cells: &[Cell<'_>], width: usize) -> Vec<Line<'static>> {
    if cells.is_empty() {
        return vec![Line::default()];
    }
    let mut lines: Vec<Vec<&Cell<'_>>> = vec![Vec::new()];
    let mut used = 0;
    for (start, end) in atoms(cells) {
        let atom = &cells[start..end];
        // Spaces at its end do not count against the line it ends.
        let visible = atom.iter().rposition(|cell| !cell.text.chars().all(char::is_whitespace));
        let body: usize = match visible {
            Some(last) => atom[..=last].iter().map(|cell| cell.width).sum(),
            None => 0,
        };
        let total: usize = atom.iter().map(|cell| cell.width).sum();
        if used > 0 && used + body > width {
            lines.push(Vec::new());
            used = 0;
            if visible.is_none() {
                continue;
            }
        }
        if body > width {
            // Wider than a whole line: cut it where it has to be.
            for cell in atom {
                if used + cell.width > width && used > 0 {
                    lines.push(Vec::new());
                    used = 0;
                }
                lines.last_mut().expect("a line").push(cell);
                used += cell.width;
            }
            continue;
        }
        lines.last_mut().expect("a line").extend(atom.iter());
        used += total;
    }
    lines.into_iter().map(|cells| line_of(&trim_end(&cells))).collect()
}

fn trim_end<'a, 'b>(cells: &[&'a Cell<'b>]) -> Vec<&'a Cell<'b>> {
    let end = cells.iter().rposition(|cell| !cell.text.chars().all(char::is_whitespace)).map_or(0, |last| last + 1);
    cells[..end].to_vec()
}

fn line_of(cells: &[&Cell<'_>]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for cell in cells {
        match spans.last_mut() {
            Some(span) if span.style == cell.style => span.content.to_mut().push_str(cell.text),
            _ => spans.push(Span::styled(cell.text.to_string(), cell.style)),
        }
    }
    Line::from(spans)
}

/// The width of a line as laid out.
pub fn line_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(|span| width(&span.content)).sum()
}
