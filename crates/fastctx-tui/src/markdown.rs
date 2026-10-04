//! Markdown laid out in terminal cells, using the same line breaks as the rest of the TUI.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::num::NonZeroUsize;
use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::design::{Look, Role};
use crate::text;

mod highlight;
mod table;

pub use highlight::Memo;

/// Renders a Markdown snapshot into owned, styled lines. A zero width produces no lines.
///
/// Code, HTML and mathematics remain readable source; images expose their alt text and address.
/// Tables start at their natural column widths, wrap when space is scarce, and become labeled
/// records when the columns cannot fit. An indivisible grapheme wider than the available space
/// still occupies its natural terminal width. Quotes and code retain their specified gutters.
///
/// This function has no streaming state. Markdown that changes the interpretation of earlier
/// input (such as a setext underline or a reference definition) necessarily changes those lines.
pub fn render(markdown: &str, width: usize, look: &Look) -> Vec<Line<'static>> {
    render_remembering(markdown, width, look, &mut Memo::default())
}

/// `render`, for a text laid out again as it grows or as the width changes:
/// `memo` keeps how its code was coloured, so only what changed is read again.
pub fn render_remembering(markdown: &str, width: usize, look: &Look, memo: &mut Memo) -> Vec<Line<'static>> {
    if width == 0 || markdown.is_empty() {
        return Vec::new();
    }
    let prepared = prepare_math(markdown);
    let events: Vec<_> = Parser::new_ext(&prepared, options()).into_offset_iter().collect();
    let mut ends = vec![0; events.len()];
    let mut stack = Vec::new();
    for (index, (event, _)) in events.iter().enumerate() {
        match event {
            Event::Start(_) => stack.push(index),
            Event::End(_) => ends[stack.pop().expect("balanced Markdown events")] = index,
            _ => {}
        }
    }
    let renderer = Renderer {
        source: markdown,
        prepared: &prepared,
        events,
        ends,
        look,
        memo: RefCell::new(std::mem::take(memo)),
        code_blocks: Cell::new(0),
    };
    let lines = renderer.blocks(0..renderer.events.len(), width, false, false).lines;
    *memo = renderer.memo.into_inner();
    memo.keep(renderer.code_blocks.get());
    lines
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
        | Options::ENABLE_DEFINITION_LIST
}

struct Renderer<'a> {
    source: &'a str,
    prepared: &'a str,
    events: Vec<(Event<'a>, Range<usize>)>,
    ends: Vec<usize>,
    look: &'a Look,
    memo: RefCell<Memo>,
    /// Code blocks met so far, which numbers each in `memo`.
    code_blocks: Cell<usize>,
}

#[derive(Default)]
struct Block {
    lines: Vec<Line<'static>>,
    before: usize,
    after: usize,
}

impl Block {
    fn prose(lines: Vec<Line<'static>>) -> Self {
        Self { lines, before: 1, after: 1 }
    }

    fn append(&mut self, block: Self) {
        if block.lines.is_empty() {
            return;
        }
        if self.lines.is_empty() {
            self.before = block.before;
        } else {
            self.lines.extend((0..self.after.max(block.before)).map(|_| Line::default()));
        }
        self.after = block.after;
        self.lines.extend(block.lines);
    }
}

impl Renderer<'_> {
    fn style(&self, role: Role, quoted: bool) -> Style {
        self.look.text(if quoted { Role::Muted } else { role })
    }

    fn blocks(&self, range: Range<usize>, width: usize, quoted: bool, compact: bool) -> Block {
        let mut output = Block::default();
        let mut index = range.start;
        while index < range.end {
            let (event, _) = &self.events[index];
            let end = self.ends[index];
            let inner = index + 1..end;
            let mut block = match event {
                Event::Start(Tag::Paragraph) => Block::prose(self.prose(inner, width, quoted, Modifier::empty())),
                Event::Start(Tag::Heading { level, .. }) => {
                    let mut lines = self.prose(inner, width, quoted, Modifier::BOLD);
                    let role =
                        if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) { Role::Text } else { Role::Muted };
                    for span in lines.iter_mut().flat_map(|line| &mut line.spans) {
                        span.style = self.style(role, quoted).add_modifier(span.style.add_modifier | Modifier::BOLD);
                    }
                    let (before, after) = match level {
                        HeadingLevel::H1 => (2, 1),
                        HeadingLevel::H2 => (1, 1),
                        _ => (1, 0),
                    };
                    Block { lines, before, after }
                }
                Event::Start(Tag::CodeBlock(kind)) => self.code(inner, kind, width, quoted),
                Event::Start(Tag::HtmlBlock) => {
                    let mut raw = String::new();
                    for (event, _) in &self.events[inner] {
                        if let Event::Html(value) | Event::Text(value) = event {
                            raw.push_str(value);
                        }
                    }
                    Block::prose(literal_lines(
                        raw.strip_suffix('\n').unwrap_or(&raw),
                        self.style(Role::Text, quoted),
                        width,
                    ))
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    let indent = 2;
                    let mut block = self.blocks(inner, width.saturating_sub(indent).max(1), true, false);
                    indent_lines(&mut block.lines, "│ ", self.look.text(Role::Faint));
                    block.before = 1;
                    block.after = 1;
                    block
                }
                Event::Start(Tag::List(start)) => self.list(inner, *start, width, quoted),
                Event::Start(Tag::Table(alignments)) => self.table(inner, alignments, width, quoted),
                Event::Start(Tag::FootnoteDefinition(label)) => {
                    let prefix = format!("[^{label}] ");
                    self.prefixed(inner, &prefix, width, quoted, false)
                }
                Event::Start(Tag::DefinitionListDefinition) => self.prefixed(inner, "  ", width, quoted, false),
                Event::Start(Tag::DefinitionListTitle) => {
                    Block::prose(self.prose(inner, width, quoted, Modifier::BOLD))
                }
                Event::Start(Tag::Item | Tag::DefinitionList) => self.blocks(inner, width, quoted, false),
                Event::Rule => {
                    let mut lines = text::wrap_plain("·  ·  ·", self.style(Role::Faint, quoted), width);
                    for line in &mut lines {
                        let padding = width.saturating_sub(text::line_width(line)) / 2;
                        line.spans.insert(0, Span::styled(" ".repeat(padding), self.style(Role::Faint, quoted)));
                    }
                    Block::prose(lines)
                }
                _ => {
                    let start = index;
                    while index < range.end && !is_block(&self.events[index].0) {
                        index += match self.events[index].0 {
                            Event::Start(_) => self.ends[index] - index + 1,
                            _ => 1,
                        };
                    }
                    if index == start {
                        index += 1;
                    }
                    output.append(Block {
                        lines: self.prose(start..index, width, quoted, Modifier::empty()),
                        before: 0,
                        after: 0,
                    });
                    continue;
                }
            };
            if compact {
                block.before = 0;
                block.after = 0;
            }
            output.append(block);
            index = if matches!(event, Event::Start(_)) { end + 1 } else { index + 1 };
        }
        output
    }

    fn prose(&self, range: Range<usize>, width: usize, quoted: bool, modifiers: Modifier) -> Vec<Line<'static>> {
        let spans = self.inline(range, quoted, modifiers);
        if spans.is_empty() { Vec::new() } else { text::wrap(&spans, width) }
    }

    fn inline(&self, range: Range<usize>, quoted: bool, modifiers: Modifier) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        let mut style = self.style(Role::Text, quoted).add_modifier(modifiers);
        let mut styles = Vec::new();
        let mut links: Vec<(usize, String)> = Vec::new();
        let mut pending_break = false;
        for (event, offset) in &self.events[range] {
            if pending_break {
                if matches!(event, Event::SoftBreak | Event::HardBreak) {
                    continue;
                }
                if !matches!(event, Event::Start(_) | Event::End(_)) {
                    push(&mut spans, "\n", style);
                    pending_break = false;
                }
            }
            match event {
                Event::Start(tag) => {
                    styles.push(style);
                    match tag {
                        Tag::Strong => style = style.add_modifier(Modifier::BOLD),
                        Tag::Emphasis => style = style.add_modifier(Modifier::ITALIC),
                        Tag::Strikethrough => style = style.add_modifier(Modifier::CROSSED_OUT),
                        Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                            style = self.style(Role::Link, quoted).add_modifier(style.add_modifier);
                            links.push((spans.len(), visible(dest_url)));
                        }
                        _ => {}
                    }
                }
                Event::End(tag) => {
                    if matches!(tag, pulldown_cmark::TagEnd::Link | pulldown_cmark::TagEnd::Image) {
                        let (start, destination) = links.pop().expect("balanced link events");
                        let label: String = spans[start..].iter().map(|s: &Span<'_>| s.content.as_ref()).collect();
                        if label.is_empty() {
                            push(&mut spans, &destination, style);
                        } else if label != destination {
                            push(&mut spans, &format!(" ({destination})"), self.style(Role::Faint, quoted));
                        }
                    }
                    style = styles.pop().expect("balanced inline events");
                }
                Event::Text(value) | Event::Html(value) | Event::InlineHtml(value) => push(&mut spans, value, style),
                Event::Code(value) => {
                    push(&mut spans, value, self.style(Role::Accent, quoted).add_modifier(style.add_modifier));
                }
                Event::InlineMath(value) | Event::DisplayMath(value) => {
                    let raw = &self.source[offset.clone()];
                    let display = raw.starts_with("$$") || raw.starts_with(r"\[");
                    if display && !spans.is_empty() {
                        push(&mut spans, "\n", style);
                    }
                    let delimiter = if matches!(event, Event::InlineMath(_)) { 1 } else { 2 };
                    let value = if raw.starts_with(char::from(92)) {
                        restore_math(value, &self.prepared[offset.start + 2..offset.end - 2], &raw[2..raw.len() - 2])
                    } else {
                        value.to_string()
                    };
                    let formula = format!("{}{}{}", &raw[..delimiter], value, &raw[raw.len() - delimiter..]);
                    push(&mut spans, &formula, self.style(Role::Accent, quoted).add_modifier(style.add_modifier));
                    pending_break = display;
                }
                Event::SoftBreak => push(&mut spans, " ", style),
                Event::HardBreak => push(&mut spans, "\n", style),
                Event::FootnoteReference(label) => {
                    push(&mut spans, &format!("[^{label}]"), self.style(Role::Link, quoted))
                }
                Event::TaskListMarker(checked) => {
                    push(&mut spans, if *checked { "[x] " } else { "[ ] " }, self.style(Role::Faint, quoted));
                }
                Event::Rule => {}
            }
        }
        spans
    }

    fn prefixed(&self, range: Range<usize>, prefix: &str, width: usize, quoted: bool, compact: bool) -> Block {
        let prefix_width = text::width(prefix);
        let room = width.saturating_sub(prefix_width);
        let style = self.style(Role::Faint, quoted);
        if room < 2 {
            let mut block = self.blocks(range, width, quoted, compact);
            let mut lines = text::wrap_plain(prefix.trim_end(), style, width);
            lines.append(&mut block.lines);
            block.lines = lines;
            return block;
        }
        let marker_only =
            range.clone().next().is_some_and(|index| matches!(self.events[index].0, Event::Start(Tag::List(_))));
        let mut block = self.blocks(range, room, quoted, compact);
        if marker_only {
            block.lines.insert(0, Line::default());
        }
        if block.lines.is_empty() {
            block.lines.push(Line::default());
        }
        indent_lines(&mut block.lines, &" ".repeat(prefix_width), style);
        block.lines[0].spans[0] = Span::styled(prefix.to_owned(), style);
        block
    }

    fn list(&self, range: Range<usize>, start: Option<u64>, width: usize, quoted: bool) -> Block {
        let mut output = Block::default();
        let mut index = range.start;
        let mut number = u128::from(start.unwrap_or(1));
        while index < range.end {
            let end = self.ends[index];
            let prefix = if start.is_some() { format!("{number}. ") } else { "· ".to_owned() };
            let inner = index + 1..end;
            let mut child = inner.start;
            let mut compact = true;
            while child < inner.end {
                if matches!(self.events[child].0, Event::Start(Tag::Paragraph)) {
                    compact = false;
                    break;
                }
                child = if matches!(self.events[child].0, Event::Start(_)) { self.ends[child] + 1 } else { child + 1 };
            }
            let mut block = self.prefixed(inner, &prefix, width, quoted, compact);
            if compact {
                block.before = 0;
                block.after = 0;
            }
            output.append(block);
            index = end + 1;
            number += 1;
        }
        output.before = 1;
        output.after = 1;
        output
    }

    fn code(&self, range: Range<usize>, kind: &CodeBlockKind<'_>, width: usize, quoted: bool) -> Block {
        let mut code = String::new();
        for (event, _) in &self.events[range] {
            if let Event::Text(value) = event {
                code.push_str(value);
            }
        }
        let role = if quoted { Role::Muted } else { Role::Text };
        let base = self.look.code(Role::Text);
        let style =
            Style { fg: self.style(role, quoted).fg, ..base }.add_modifier(self.style(role, quoted).add_modifier);
        let faint = Style { fg: self.style(Role::Faint, quoted).fg, ..base }
            .add_modifier(self.style(Role::Faint, quoted).add_modifier);
        let padding = 1;
        let room = width.saturating_sub(padding * 2).max(1);
        let mut lines = Vec::new();
        let language = match kind {
            CodeBlockKind::Fenced(info) => info.split_whitespace().next(),
            CodeBlockKind::Indented => None,
        };
        if let Some(language) = language {
            for mut line in literal_lines(language, faint, room) {
                let left = room.saturating_sub(text::line_width(&line));
                line.spans.insert(0, Span::styled(" ".repeat(left), style));
                lines.push(line);
            }
        }
        let code = code.strip_suffix('\n').unwrap_or(&code);
        let block = self.code_blocks.replace(self.code_blocks.get() + 1);
        // A quoted block stays one quiet colour, like the quote around it;
        // an empty one shows only its language, whether or not it is known.
        let painted = language
            .filter(|_| !quoted && !code.is_empty())
            .and_then(|language| self.painted(block, language, code, room));
        if let Some(painted) = painted {
            lines.extend(painted);
        } else if !code.is_empty() {
            lines.extend(literal_lines(code, style, room));
        } else if lines.is_empty() {
            lines.push(Line::default());
        }
        for line in &mut lines {
            let right = width.saturating_sub(padding + text::line_width(line)).max(padding);
            line.spans.insert(0, Span::styled(" ".repeat(padding), style));
            line.spans.push(Span::styled(" ".repeat(right), style));
        }
        Block::prose(lines)
    }

    /// Code in `language`, coloured by its grammar and wrapped like any
    /// literal; `None` when no grammar knows the language.
    fn painted(&self, block: usize, language: &str, code: &str, width: usize) -> Option<Vec<Line<'static>>> {
        let source = visible(code).replace('\r', r"\r");
        let layout = text::Layout::default().with_tab_stop(NonZeroUsize::new(4).unwrap());
        let pieces = self.memo.borrow_mut().highlight(block, language, &layout.expand_tabs(&source))?;
        let mut lines = Vec::new();
        for line in pieces {
            let spans: Vec<Span<'static>> =
                line.into_iter().map(|(role, text)| Span::styled(text, self.look.code(role))).collect();
            if spans.is_empty() {
                lines.push(Line::default());
            } else {
                lines.extend(layout.wrap_preserving_spaces(&spans, width));
            }
        }
        Some(lines)
    }

    fn table(&self, range: Range<usize>, alignments: &[Alignment], width: usize, quoted: bool) -> Block {
        let mut rows = Vec::new();
        let mut index = range.start;
        while index < range.end {
            let end = self.ends[index];
            let header = matches!(self.events[index].0, Event::Start(Tag::TableHead));
            let mut cells = Vec::new();
            let mut cell = index + 1;
            while cell < end {
                let cell_end = self.ends[cell];
                cells.push(self.inline(
                    cell + 1..cell_end,
                    quoted,
                    if header { Modifier::BOLD } else { Modifier::empty() },
                ));
                cell = cell_end + 1;
            }
            rows.push(cells);
            index = end + 1;
        }
        let style = self.style(Role::Text, quoted);
        Block::prose(table::render(&rows, alignments, width, style, self.look.text(Role::Faint)))
    }
}

fn is_block(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Rule
            | Event::Start(
                Tag::Paragraph
                    | Tag::Heading { .. }
                    | Tag::CodeBlock(_)
                    | Tag::HtmlBlock
                    | Tag::BlockQuote(_)
                    | Tag::List(_)
                    | Tag::Item
                    | Tag::Table(_)
                    | Tag::FootnoteDefinition(_)
                    | Tag::DefinitionList
                    | Tag::DefinitionListTitle
                    | Tag::DefinitionListDefinition
            )
    )
}

fn indent_lines(lines: &mut [Line<'static>], prefix: &str, style: Style) {
    for line in lines {
        line.spans.insert(0, Span::styled(prefix.to_owned(), style));
    }
}

fn push(spans: &mut Vec<Span<'static>>, value: &str, style: Style) {
    if !value.is_empty() {
        spans.push(Span::styled(visible(value), style));
    }
}

fn visible(value: &str) -> String {
    let mut output = String::new();
    for ch in value.replace("\r\n", "\n").chars() {
        if ch.is_control() && !matches!(ch, '\n' | '\t') {
            output.extend(ch.escape_default());
        } else {
            output.push(ch);
        }
    }
    output
}

/// Literal source keeps its spaces, including indentation on continuation lines.
fn literal_lines(source: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    let source = visible(source).replace("\r\n", "\n").replace('\r', r"\r");
    let mut lines = Vec::new();
    // CommonMark expands tabs on the logical source line at four-cell stops.
    let layout = text::Layout::default().with_tab_stop(NonZeroUsize::new(4).unwrap());
    for raw in source.split('\n') {
        let expanded = layout.expand_tabs(raw);
        lines.extend(layout.wrap_preserving_spaces(&[Span::styled(expanded, style)], width));
    }
    lines
}

/// Parser normalization removes container prefixes and table escapes. Match the normalized
/// characters back to their byte positions to recover the original TeX operators.
fn restore_math(value: &str, prepared: &str, original: &str) -> String {
    let mut positions = prepared.char_indices();
    value
        .chars()
        .map(|ch| {
            let (offset, _) = positions
                .find(|(_, candidate)| *candidate == ch)
                .expect("math normalization only removes source characters");
            original[offset..].chars().next().expect("matching source character")
        })
        .collect()
}

/// TeX delimiters occupy two bytes, like display-math delimiters. Keeping byte offsets lets
/// the renderer recover the original notation, even inside lists, quotes and table cells.
fn prepare_math(source: &str) -> Cow<'_, str> {
    if !source.as_bytes().windows(2).any(|pair| pair[0] == 92 && matches!(pair[1], b'(' | b'[')) {
        return Cow::Borrowed(source);
    }
    let mut text_ranges = Vec::new();
    let mut stack = Vec::new();
    let mut scope = (0, source.len());
    let mut literal = false;
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                stack.push((scope, literal));
                if matches!(tag, Tag::Paragraph | Tag::Heading { .. } | Tag::Item | Tag::TableCell) {
                    scope = (range.start, range.end);
                }
                literal |= matches!(tag, Tag::CodeBlock(_) | Tag::HtmlBlock);
            }
            Event::End(_) => (scope, literal) = stack.pop().expect("balanced Markdown events"),
            Event::Text(_) if !literal => text_ranges.push((range, scope)),
            _ => {}
        }
    }
    let at = |position: usize| {
        let index = text_ranges.partition_point(|(range, _)| range.start <= position);
        index.checked_sub(1).and_then(|index| {
            let (range, scope) = &text_ranges[index];
            range.contains(&position).then_some(*scope)
        })
    };
    let bytes = source.as_bytes();
    let mut output = bytes.to_vec();
    let mut candidates = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] != 92 {
            index += 1;
            continue;
        }
        let closing = match bytes[index + 1] {
            b'(' => b')',
            b'[' => b']',
            _ => {
                index += 2;
                continue;
            }
        };
        let Some(scope) = at(index + 1) else {
            index += 2;
            continue;
        };
        let start = index;
        let mut end = index + 2;
        index += 2;
        while end + 1 < scope.1 {
            if bytes[end] == 92 && bytes[end + 1] == closing && at(end + 1) == Some(scope) {
                output[start..start + 2].copy_from_slice(b"$$");
                output[end..end + 2].copy_from_slice(b"$$");
                // Dollar signs and braces belong to TeX here; mask only the bytes that affect
                // pulldown-cmark's dollar-math matching, then restore them after parsing.
                for byte in &mut output[start + 2..end] {
                    if matches!(*byte, b'$' | b'{' | b'}') {
                        *byte = b'x';
                    }
                }
                index = end + 2;
                candidates.push(start..index);
                break;
            }
            end += if bytes[end] == 92 { 2 } else { 1 };
        }
    }
    if candidates.is_empty() {
        return Cow::Borrowed(source);
    }
    // Revert substitutions that do not become exactly one math event. Each pass removes at
    // least one candidate or finishes, so termination follows from the number of candidates.
    loop {
        let prepared = std::str::from_utf8(&output).expect("only ASCII bytes were replaced");
        let recognized: Vec<_> = Parser::new_ext(prepared, options())
            .into_offset_iter()
            .filter_map(|(event, range)| matches!(event, Event::DisplayMath(_)).then_some(range))
            .collect();
        let previous = candidates.len();
        candidates.retain(|range| {
            if recognized.contains(range) {
                true
            } else {
                output[range.clone()].copy_from_slice(&bytes[range.clone()]);
                false
            }
        });
        if candidates.len() == previous {
            break;
        }
    }
    Cow::Owned(String::from_utf8(output).expect("only ASCII bytes were replaced"))
}
