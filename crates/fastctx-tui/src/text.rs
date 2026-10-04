//! Grapheme-safe measurement, wrapping, clipping and painting.
//!
//! The default is Ratatui's cell model, not a promise about every terminal.
//! Classic conhost and VS Code's Unicode 11 mode disagree with it on emoji
//! and combining characters. Supply terminal measurements to [`Layout::new`]
//! and use that same layout to paint: [`Layout::write_line`] records those
//! widths in the buffer, preventing Ratatui's diff from reinterpreting them.

use std::borrow::Cow;
use std::num::{NonZeroU16, NonZeroUsize};

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_linebreak::{BreakClass, break_property, linebreaks};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// One terminal's measurements and the text area's tab stops.
///
/// Tabs become spaces before painting, relative to each output line, every
/// eight cells by default. Terminal tab settings and the area's screen
/// position cannot change the result. Measurement and painting include spaces;
/// prose wrapping removes trailing whitespace, while literal wrapping keeps it.
pub struct Layout<'a> {
    measure: &'a dyn Fn(&str) -> usize,
    tab_stop: NonZeroUsize,
}

impl Default for Layout<'static> {
    fn default() -> Self {
        Self::new(&ratatui_width)
    }
}

impl<'a> Layout<'a> {
    /// Supplies the width of one extended grapheme cluster in this terminal.
    ///
    /// The function must be deterministic for the layout's lifetime, including
    /// for `…` and a dotted-circle representation of an isolated combining
    /// mark. Tabs, line endings and controls are handled here. This does not
    /// probe a terminal or infer its settings from its name.
    pub const fn new(measure: &'a dyn Fn(&str) -> usize) -> Self {
        Self { measure, tab_stop: NonZeroUsize::new(8).unwrap() }
    }

    /// Sets the interval between tab stops in the text area.
    pub const fn with_tab_stop(mut self, tab_stop: NonZeroUsize) -> Self {
        self.tab_stop = tab_stop;
        self
    }

    /// The widest logical line, including whitespace used by source and padding.
    pub fn width(&self, text: &str) -> usize {
        self.line_width(&Line::from(Span::raw(text)))
    }

    /// The cell advance of the widest logical line starting at `column`.
    /// This accounts for tabs when measuring fragments of an existing line.
    pub fn width_at(&self, text: &str, column: usize) -> usize {
        let source = Source::new(&[Span::raw(text)]);
        self.cells(&source)
            .split(Cell::newline)
            .map(|cells| cells.iter().fold(column, |at, cell| at + self.cell_width(cell, at)) - column)
            .max()
            .unwrap_or(0)
    }

    /// Expands tabs at this layout's stops on each logical line, preserving
    /// every other source character. Call before literal wrapping when tab
    /// stops belong to the source line rather than its visual continuations.
    pub fn expand_tabs(&self, text: &str) -> String {
        let mut output = String::new();
        let mut column = 0;
        for grapheme in text.graphemes(true) {
            if is_newline(grapheme) {
                output.push_str(grapheme);
                column = 0;
            } else {
                let cells = self.width_at(grapheme, column);
                if grapheme == "\t" {
                    output.push_str(&" ".repeat(cells));
                } else {
                    output.push_str(grapheme);
                }
                column += cells;
            }
        }
        output
    }

    /// Measures across span boundaries; style changes cannot split a cluster.
    pub fn line_width(&self, line: &Line<'_>) -> usize {
        let source = Source::new(&line.spans);
        self.cells(&source).split(Cell::newline).map(|cells| self.extent(cells)).max().unwrap_or(0)
    }

    /// Fits the first logical line, with `…` if content was omitted.
    /// Clusters are indivisible. Returns empty if the ellipsis does not fit.
    /// Spaces are retained; tabs expand even when clipping is unnecessary.
    pub fn fit(&self, text: &str, max: usize) -> String {
        self.fit_line(&Line::from(Span::raw(text)), max).spans.iter().map(|span| span.content.as_ref()).collect()
    }

    /// Styled [`Self::fit`]. A cluster takes its first code point's style.
    pub fn fit_line(&self, line: &Line<'_>, max: usize) -> Line<'static> {
        let source = Source::new(&line.spans);
        let cells = self.cells(&source);
        let first = cells.iter().position(Cell::newline).unwrap_or(cells.len());
        let content = &cells[..first];
        let cut = first < cells.len() || self.extent(content) > max;
        let marker = (self.measure)("…");
        let mut result = if !cut {
            self.line_of(content)
        } else if max == 0 || marker > max {
            Line::default()
        } else {
            let mut used = 0;
            let end = content
                .iter()
                .position(|cell| {
                    used += self.cell_width(cell, used);
                    used > max - marker
                })
                .unwrap_or(content.len());
            let kept = &content[..end];
            let mut result = self.line_of(kept);
            let style = kept.last().or(content.first()).map_or(Style::default(), |cell| cell.style);
            result.spans.push(Span::styled("…", style));
            result
        };
        result.style = line.style;
        result.alignment = line.alignment;
        result
    }

    /// Unicode line breaking with Chinese punctuation constraints.
    /// Western words fitting on a whole line never split; longer words may
    /// break between clusters, still respecting opening/closing punctuation.
    /// Whitespace at a line break or paragraph end is discarded before painting.
    ///
    /// An indivisible cluster or punctuation-bound group wider than `max`
    /// occupies an overwide line. Preserving text and the punctuation rules
    /// within a smaller rectangle is impossible; painting clips that line.
    /// Zero width produces no lines. Explicit paragraph boundaries are
    /// honored even if the input itself starts/ends with forbidden marks.
    pub fn wrap(&self, spans: &[Span<'_>], max: usize) -> Vec<Line<'static>> {
        self.wrap_impl(spans, max, false)
    }

    /// Wraps literal source with the same word and punctuation rules as prose,
    /// retaining indentation, inter-word spaces and trailing spaces. Long runs
    /// of spaces may continue on the next line; they are never dropped.
    pub fn wrap_preserving_spaces(&self, spans: &[Span<'_>], max: usize) -> Vec<Line<'static>> {
        self.wrap_impl(spans, max, true)
    }

    /// The widest run kept together by ordinary wrapping, excluding its
    /// trailing whitespace. Uses the same graphemes and boundaries as `wrap`.
    pub fn unbreakable_width(&self, text: &str) -> usize {
        let source = Source::new(&[Span::raw(text)]);
        let cells = self.cells(&source);
        let mut widest = 0;
        for paragraph in cells.split(Cell::newline).filter(|paragraph| !paragraph.is_empty()) {
            let (normal, _) = self.boundaries(paragraph, &source.text, usize::MAX, false);
            let mut start = 0;
            for (end, allowed) in normal.into_iter().enumerate().skip(1) {
                if allowed {
                    widest = widest.max(self.extent(trim_end(&paragraph[start..end])));
                    start = end;
                }
            }
        }
        widest
    }

    fn wrap_impl(&self, spans: &[Span<'_>], max: usize, preserve_spaces: bool) -> Vec<Line<'static>> {
        if max == 0 {
            return Vec::new();
        }
        let source = Source::new(spans);
        let cells = self.cells(&source);
        let mut lines = Vec::new();
        for paragraph in cells.split(Cell::newline) {
            self.wrap_paragraph(paragraph, &source.text, max, preserve_spaces, &mut lines);
        }
        lines
    }

    /// Paints the first logical line, clipped to `area`, returning the first
    /// unused column. Alignment belongs to the caller.
    ///
    /// Use this instead of `Buffer::set_stringn` for terminal-specific widths.
    /// Placement and buffer diff use the same measured width; continuation
    /// cells are reset. No terminal escapes are stored in the buffer.
    pub fn write_line(&self, buffer: &mut Buffer, area: Rect, line: &Line<'_>) -> u16 {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return area.x;
        }
        let source = Source::new(&line.spans);
        let cells = self.cells(&source);
        let end = cells.iter().position(Cell::newline).unwrap_or(cells.len());
        let mut x = area.x;
        for cell in &cells[..end] {
            let columns = self.cell_width(cell, usize::from(x - area.x));
            if columns > usize::from(area.right() - x) {
                break;
            }
            let Some(columns) = NonZeroU16::new(columns as u16) else { continue };
            let symbol =
                if cell.text == "\t" { Cow::Owned(" ".repeat(usize::from(columns.get()))) } else { cell.text.clone() };
            buffer[(x, area.y)]
                .set_symbol(&symbol)
                .set_style(line.style.patch(cell.style))
                .set_diff_option(CellDiffOption::ForcedWidth(columns));
            let next = x + columns.get();
            for covered in x + 1..next {
                buffer[(covered, area.y)].reset();
                // ForcedWidth skips Ratatui's trailing-cell invalidation.
                // Record coverage so a later uncovered blank differs from
                // this cell and clears residual glyphs on legacy terminals.
                buffer[(covered, area.y)].set_diff_option(CellDiffOption::Skip);
            }
            x = next;
        }
        x
    }

    fn cells<'s>(&self, source: &'s Source) -> Vec<Cell<'s>> {
        let mut breaks = linebreaks(&source.text).peekable();
        let mut runs = source.runs.iter();
        let mut run = runs.next();
        let mut cells = Vec::new();
        for (start, grapheme) in source.text.grapheme_indices(true) {
            while run.is_some_and(|(end, _)| *end <= start) {
                run = runs.next();
            }
            let end = start + grapheme.len();
            while breaks.peek().is_some_and(|(at, _)| *at < end) {
                breaks.next();
            }
            let break_after = breaks.peek().is_some_and(|(at, _)| *at == end);
            let style = run.map_or(Style::default(), |(_, style)| *style);
            let special = grapheme == "\t" || is_newline(grapheme);
            // Match Ratatui's control filtering. An ESC in content must not
            // become an instruction to the terminal.
            if !special && grapheme.contains(char::is_control) {
                continue;
            }
            let mut text = Cow::Borrowed(grapheme);
            if !special
                && grapheme.chars().next().is_some_and(|c| break_property(c as u32) == BreakClass::CombiningMark)
            {
                // An isolated mark needs a base cell, otherwise its extent
                // depends on whatever happened to be painted before it.
                text = Cow::Owned(format!("◌{grapheme}"));
            }
            let columns = if special { 0 } else { (self.measure)(&text) };
            cells.push(Cell { text, style, columns, start, end, break_after });
        }
        cells
    }

    fn cell_width(&self, cell: &Cell<'_>, column: usize) -> usize {
        if cell.text == "\t" { self.tab_stop.get() - column % self.tab_stop.get() } else { cell.columns }
    }

    fn extent(&self, cells: &[Cell<'_>]) -> usize {
        cells.iter().fold(0, |column, cell| column + self.cell_width(cell, column))
    }

    fn line_of(&self, cells: &[Cell<'_>]) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut column = 0;
        for cell in cells {
            let columns = self.cell_width(cell, column);
            let text = if cell.text == "\t" { Cow::Owned(" ".repeat(columns)) } else { cell.text.clone() };
            match spans.last_mut() {
                Some(span) if span.style == cell.style => span.content.to_mut().push_str(&text),
                _ => spans.push(Span::styled(text.into_owned(), cell.style)),
            }
            column += columns;
        }
        Line::from(spans)
    }

    fn wrap_paragraph(
        &self,
        cells: &[Cell<'_>],
        source: &str,
        max: usize,
        preserve_spaces: bool,
        lines: &mut Vec<Line<'static>>,
    ) {
        let cells = if preserve_spaces { cells } else { trim_end(cells) };
        if cells.is_empty() {
            lines.push(Line::default());
            return;
        }
        let (normal, emergency) = self.boundaries(cells, source, max, preserve_spaces);
        let mut start = 0;
        while start < cells.len() {
            let mut used = 0;
            let mut visible = 0;
            let mut last_normal = None;
            let mut last_emergency = None;
            let mut end = start;
            while end < cells.len() {
                used += self.cell_width(&cells[end], used);
                if preserve_spaces || !cells[end].space() {
                    visible = used;
                }
                if visible > max {
                    break;
                }
                end += 1;
                if visible > 0 {
                    if normal[end] {
                        last_normal = Some(end);
                    }
                    if emergency[end] {
                        last_emergency = Some(end);
                    }
                }
            }
            if end < cells.len() {
                end = last_normal.or(last_emergency).unwrap_or_else(|| {
                    // E.g. "中。" cannot obey both punctuation rules and a
                    // two-cell bound. Preserve the smallest indivisible group.
                    (end + 1..=cells.len()).find(|&at| emergency[at]).expect("paragraph end is a boundary")
                });
            }
            let kept = &cells[start..end];
            lines.push(self.line_of(if preserve_spaces { kept } else { trim_end(kept) }));
            start = end;
            while !preserve_spaces && start < cells.len() && cells[start].space() {
                start += 1;
            }
        }
    }

    fn boundaries(
        &self,
        cells: &[Cell<'_>],
        source: &str,
        max: usize,
        preserve_spaces: bool,
    ) -> (Vec<bool>, Vec<bool>) {
        let n = cells.len();
        let mut normal = vec![false; n + 1];
        let mut emergency = vec![false; n + 1];
        let mut next_ink = vec![n; n + 1];
        for at in (0..n).rev() {
            next_ink[at] = if cells[at].space() { next_ink[at + 1] } else { at };
        }
        let mut previous = None;
        for at in 1..n {
            if !cells[at - 1].space() {
                previous = Some(at - 1);
            }
            let left = previous.map(|index| &cells[index]);
            let right = cells.get(next_ink[at]);
            let punctuation_ok =
                !left.is_some_and(|cell| opens(&cell.text)) && !right.is_some_and(|cell| closes(&cell.text));
            normal[at] = cells[at - 1].break_after && punctuation_ok;
            emergency[at] = punctuation_ok
                && (preserve_spaces || !cells[at].space())
                && !glue(&cells[at - 1].text)
                && !glue(&cells[at].text);
        }
        let offset = cells[0].start;
        let paragraph = &source[offset..cells[n - 1].end];
        for (start, word) in paragraph.unicode_word_indices() {
            if word.chars().any(is_east_asian) || self.width(word) > max {
                continue;
            }
            let start = offset + start;
            let end = start + word.len();
            let first = cells.partition_point(|cell| cell.start <= start);
            for at in first..n {
                if cells[at].start >= end {
                    break;
                }
                normal[at] = false;
                emergency[at] = false;
            }
        }
        normal[n] = true;
        emergency[n] = true;
        (normal, emergency)
    }
}

struct Source {
    text: String,
    runs: Vec<(usize, Style)>,
}

impl Source {
    fn new(spans: &[Span<'_>]) -> Self {
        let mut text = String::new();
        let mut runs = Vec::with_capacity(spans.len());
        for span in spans {
            text.push_str(&span.content);
            runs.push((text.len(), span.style));
        }
        Self { text, runs }
    }
}

struct Cell<'a> {
    text: Cow<'a, str>,
    style: Style,
    columns: usize,
    start: usize,
    end: usize,
    break_after: bool,
}

impl Cell<'_> {
    fn space(&self) -> bool {
        self.text.chars().all(char::is_whitespace)
    }
    fn newline(&self) -> bool {
        is_newline(&self.text)
    }
}

fn is_newline(text: &str) -> bool {
    matches!(text, "\n" | "\r\n" | "\r" | "\u{0085}" | "\u{2028}" | "\u{2029}")
}

fn trim_end<'a, 'b>(cells: &'a [Cell<'b>]) -> &'a [Cell<'b>] {
    let end = cells.iter().rposition(|cell| !cell.space()).map_or(0, |last| last + 1);
    &cells[..end]
}

fn ratatui_width(grapheme: &str) -> usize {
    // Ratatui 0.30.2's halfwidth sound-mark correction, without its u16
    // narrowing. A large cluster must be clipped, not wrap its width to zero.
    grapheme.width() + grapheme.chars().filter(|c| matches!(c, '\u{ff9e}' | '\u{ff9f}')).count()
}

fn closes(text: &str) -> bool {
    text.chars().next().is_some_and(|c| {
        matches!(
            break_property(c as u32),
            BreakClass::ClosePunctuation
                | BreakClass::CloseParenthesis
                | BreakClass::Exclamation
                | BreakClass::InfixSeparator
                | BreakClass::NonStarter
                | BreakClass::ConditionalJapaneseStarter
        ) || "’”»›、。，．".contains(c)
    })
}

fn opens(text: &str) -> bool {
    text.chars().next().is_some_and(|c| break_property(c as u32) == BreakClass::OpenPunctuation || "‘“«‹".contains(c))
}

fn glue(text: &str) -> bool {
    text.chars().any(|c| matches!(break_property(c as u32), BreakClass::NonBreakingGlue | BreakClass::WordJoiner))
}

fn is_east_asian(c: char) -> bool {
    matches!(
        break_property(c as u32),
        BreakClass::Ideographic
            | BreakClass::ConditionalJapaneseStarter
            | BreakClass::HangulLvSyllable
            | BreakClass::HangulLvtSyllable
            | BreakClass::HangulLJamo
            | BreakClass::HangulVJamo
            | BreakClass::HangulTJamo
    )
}

/// The widest logical line under Ratatui's cell model, including spaces.
pub fn width(text: &str) -> usize {
    Layout::default().width(text)
}

/// Measures a fragment at its actual starting column, including tab advances.
pub fn width_at(text: &str, column: usize) -> usize {
    Layout::default().width_at(text, column)
}

/// Fits the first logical line under Ratatui's cell model; see [`Layout::fit`].
pub fn fit(text: &str, max: usize) -> String {
    Layout::default().fit(text, max)
}

/// The first line of `text`, with `…` when more follows.
pub fn first_line(text: &str) -> String {
    let mut lines = text.trim().lines();
    let first = lines.next().unwrap_or_default().trim_end().to_string();
    if lines.next().is_some() { format!("{first} …") } else { first }
}

/// Wraps under Ratatui's cell model; see [`Layout::wrap`] for overwide groups.
pub fn wrap(spans: &[Span<'_>], width: usize) -> Vec<Line<'static>> {
    Layout::default().wrap(spans, width)
}

/// Wraps literal source without dropping whitespace; see [`Layout::wrap_preserving_spaces`].
pub fn wrap_preserving_spaces(spans: &[Span<'_>], width: usize) -> Vec<Line<'static>> {
    Layout::default().wrap_preserving_spaces(spans, width)
}

/// The widest ordinary wrapping unit, excluding trailing whitespace.
pub fn unbreakable_width(text: &str) -> usize {
    Layout::default().unbreakable_width(text)
}

/// Plain text wrapped in one style.
pub fn wrap_plain(text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    wrap(&[Span::styled(text, style)], width)
}

/// The width after joining spans, including whitespace used by source and padding.
pub fn line_width(line: &Line<'_>) -> usize {
    Layout::default().line_width(line)
}
