//! Streaming file text after a shared encoding-evidence pass.
//!
//! At most one encoded line is decoded at a time: lines inside the
//! window and the budget are kept, and every other line is only counted, so
//! the head note can say `of N` for a file of any size. A line longer than
//! what is left of the budget is shown up to there; the head note says which
//! characters of it are shown, in the numbering `--chars` takes, and a note
//! after the lines says what of the selection was not shown.

use std::fs::{File, Metadata};
use std::io::{self, Seek as _};
use std::path::Path;

use fastctx_types::residents::Seen;
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};

use super::decode::Hashed;
use super::{Options, Section, head, hex, plural};
use crate::encoding::file::{Plan, content, excerpt, visit};
pub(crate) use crate::text::visible;

#[allow(clippy::too_many_arguments)]
pub fn look(
    path: &Path,
    options: &Options,
    budget: usize,
    mut file: File,
    start: Vec<u8>,
    metadata: &Metadata,
    limits: super::explore::Limits,
) -> io::Result<(Section, Option<Seen>)> {
    file.rewind()?;
    let (plan, evidence_hash) = {
        let mut scan = Hashed { reader: &mut file, hash: Sha256::new() };
        let plan = match Plan::inspect(&mut scan, options.encoding.as_deref()) {
            Ok(plan) => plan,
            Err(facts) => {
                let kind = kind(&start).map(|kind| format!("; {kind}")).unwrap_or_default();
                let mut section = Section::note(path, options, &format!("not shown: {facts}{kind}"));
                section.failed = kind.is_empty() || options.encoding.is_some();
                return Ok((section, None));
            }
        };
        std::io::copy(&mut scan, &mut std::io::sink())?;
        (plan, scan.hash.finalize())
    };
    file.rewind()?;
    let mut stream = Hashed { reader: file, hash: Sha256::new() };
    let mut reader = Reader::new(
        budget_for_body(path, options, budget).min(if options.exploring() { super::explore::WHOLE } else { budget }),
        options.lines,
        options.chars,
    );
    let mut exploring = options.exploring().then(|| super::explore::Exploring::new(path, limits));
    let mut labels = std::collections::BTreeSet::new();
    let mut readonly = false;
    let mut previous = None;
    let mut first = None;
    let mut first_shown = None;
    if let Err(facts) = visit(&mut stream, &plan, |line| {
        if let Some(exploring) = &mut exploring {
            exploring.begin_line(line.bytes.end as u64 == metadata.len());
        }
        // A line of ASCII reads alike in every encoding that keeps ASCII,
        // so only the others say which encoding a file uses.
        let ascii = line.text.is_ascii().map_err(|e| e.to_string())?;
        let telling = line.codec.filter(|codec| !codec.ascii_compatible() || !ascii);
        let label = telling.map(|codec| codec.label());
        if let Some(label) = label {
            labels.insert(label);
            first.get_or_insert((label, line.basis));
        }
        if line.codec.is_none() {
            readonly = true;
        }
        if reader.wanted() {
            let beginning = first_shown.is_none();
            if let Some(label) = label {
                first_shown.get_or_insert((label, line.basis));
            }
            if let Some(issue) = &line.issue {
                reader.note(&format!("[the next line is shown as bytes and cannot be edited: {issue}]"));
            } else if let Some(label) = label
                && !beginning
                && previous != Some(label)
            {
                reader.note(&format!("[from here: {label}{}]", doubt(line.basis)));
            }
        }
        if label.is_some() {
            previous = label;
        }
        if let Some(exploring) = &mut exploring {
            if let Some(issue) = &line.issue {
                exploring.note(&format!("[the next line is shown as bytes and cannot be edited: {issue}]"));
            } else if let Some(label) = label {
                exploring.note(&format!("[from here: {label}{}]", doubt(line.basis)));
            }
        }
        if line.issue.is_some() {
            let ending = if line.text.ends_with(b"\r\n").map_err(|e| e.to_string())? {
                "\r\n"
            } else if line.text.ends_with(b"\n").map_err(|e| e.to_string())? {
                "\n"
            } else {
                ""
            };
            let text = format!("{}{ending}", excerpt(&content(&line.text)?)?);
            reader.take(&text);
            if let Some(exploring) = &mut exploring {
                exploring.take(&text);
            }
        } else {
            line.text
                .utf8(|text| {
                    reader.take(text);
                    if let Some(exploring) = &mut exploring {
                        exploring.take(text);
                    }
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }) {
        return Ok((Section::error(path, options, &format!("not shown: {facts}")), None));
    }
    let read_hash = stream.hash.finalize();
    if read_hash != evidence_hash {
        return Ok((
            Section::error(
                path,
                options,
                "not shown: file bytes changed between encoding selection and decoding; view the file again",
            ),
            None,
        ));
    }
    let read = reader.finish();

    let mut facts = String::new();
    if labels.len() > 1 {
        if let Some((label, basis)) = first_shown.or(first) {
            facts.push_str(&format!("mixed encodings, starting with {label}{}; ", doubt(basis)));
        }
    } else if let Some((label, basis)) = first
        && label != "UTF-8"
    {
        facts.push_str(&format!("{label}{}; ", doubt(basis)));
    }
    if readonly {
        facts.push_str("some lines shown as bytes; ");
    }
    facts.push_str(&line_endings(read.crlf, read.lf));
    let mut section = section(path, options, &read, &facts);
    let large = exploring.as_ref().is_some_and(|exploring| !exploring.fits(path, options, &facts, read.total_lines));
    if let Some(exploring) = exploring
        && large
    {
        section = exploring.finish(path, options, budget, &read, &facts);
    }
    let whole = read.shown_lines == read.total_lines
        && read.cut.is_none()
        && options.chars.is_none()
        && !section.partial
        && !large;
    section.whole = whole;
    let seen = Seen {
        path: path.to_string_lossy().into_owned(),
        at: Timestamp::now(),
        sha256: (!options.exploring() || whole).then(|| hex(&read_hash)),
        size: metadata.len(),
        modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
        encoding: if readonly || labels.len() > 1 { None } else { labels.first().map(|label| (*label).into()) },
        encoding_selection: options.encoding.clone(),
    };
    let shown = section.shown;
    Ok((section, shown.then_some(seen)))
}

pub(super) fn section(path: &Path, options: &Options, read: &Read, encoding_facts: &str) -> Section {
    let mut facts = encoding_facts.to_owned();
    let total = read.total_lines;
    let whole = read.shown_lines == read.total_lines && read.cut.is_none() && options.chars.is_none();
    match (&read.cut, read.shown_lines) {
        (None, 0) if read.total_lines == 0 => facts.push_str("empty"),
        (None, _) if whole => facts.push_str(&plural(read.total_lines, "line", "lines")),
        (None, 0) => facts.push_str(&format!("no lines in the range asked; the file has {total} lines")),
        (None, shown) => {
            let first = read.first_shown;
            facts.push_str(&format!("lines {first}-{} of {total}", first + shown - 1));
            if let Some((from, to)) = options.chars {
                facts.push_str(&match shown {
                    1 if from > read.last_length => format!("; the line has {} characters", read.last_length),
                    1 => format!("; characters {from}-{} of {}", to.min(read.last_length), read.last_length),
                    _ => format!("; characters {from}-{to} of each line"),
                });
            }
        }
        (Some(cut), shown) => {
            let first = read.first_shown;
            if shown > 0 {
                facts.push_str(&format!("lines {first}-{} of {total} shown; ", first + shown - 1));
            }
            facts.push_str(&cut_facts(cut));
        }
    }
    let mut text = format!("{}\n", head(path, options, &facts));
    text.push_str(&read.body);
    if let Some(rest) = rest(read, options) {
        text.push_str(&rest);
    }
    let cost = text.len();
    let shown = read.shown_lines > 0 || read.cut.is_some() || read.total_lines == 0;
    Section {
        text,
        cost,
        shown,
        partial: read.cut.is_some() || read.out_of_budget,
        failed: false,
        whole: false,
        pictures: Vec::new(),
    }
}

/// Which characters of a line too long to show were shown.
pub(super) fn cut_facts(cut: &Cut) -> String {
    format!("line {}: characters {}-{} of {} shown", cut.line, cut.from, cut.to, cut.length)
}

/// What of the lines asked for was not shown, said again after them: a
/// reader that went through all the text meets it last. Facts only; how to
/// read the rest is the reader's to choose.
fn rest(read: &Read, options: &Options) -> Option<String> {
    let (first, last) = options.lines.unwrap_or((1, read.total_lines));
    let end = last.min(read.total_lines);
    let lines = |from: u64| if from == end { format!("line {from}") } else { format!("lines {from}-{end}") };
    match &read.cut {
        Some(cut) => {
            let to = options.chars.map_or(cut.length, |(_, to)| to.min(cut.length));
            let later = if cut.line < end { format!(", and {}", lines(cut.line + 1)) } else { String::new() };
            Some(format!("=== line {}: characters {}-{to}{later} not shown ===\n", cut.line, cut.to + 1))
        }
        None if read.out_of_budget => {
            let next = if read.shown_lines > 0 { read.first_shown + read.shown_lines } else { first };
            Some(format!("=== {} not shown ===\n", lines(next)))
        }
        None => None,
    }
}

/// What a file's first bytes say it is, for a binary file.
fn kind(start: &[u8]) -> Option<&'static str> {
    let kinds: [(&[u8], &str); 9] = [
        (b"PK\x03\x04", "a ZIP archive (also .docx, .xlsx, .jar)"),
        (b"\x1f\x8b", "gzip-compressed"),
        (b"7z\xbc\xaf\x27\x1c", "a 7z archive"),
        (b"\x28\xb5\x2f\xfd", "zstd-compressed"),
        (b"\x7fELF", "an ELF executable or library"),
        (b"MZ", "a Windows executable or DLL"),
        (b"SQLite format 3\0", "an SQLite database"),
        (b"\0asm", "WebAssembly"),
        (b"\xcf\xfa\xed\xfe", "a Mach-O executable"),
    ];
    kinds.iter().find(|(magic, _)| start.starts_with(magic)).map(|(_, kind)| *kind)
}

/// What a reader should know of how an encoding was chosen: only that it
/// was guessed, when it was, since then the text may need `--encoding`.
fn doubt(basis: &str) -> &'static str {
    if crate::encoding::guessed(basis) { ", guessed from its bytes" } else { "" }
}

/// The budget left for the lines once the head note is paid for, the head
/// at its longest: the path, the machine, and numbers of up to twenty
/// digits in the longest form of the facts.
fn budget_for_body(path: &Path, options: &Options, budget: usize) -> usize {
    // The longest encoding facts emitted, an encoding name at its longest.
    let encoding = "mixed encodings, starting with x-mac-cyrillic, guessed from its bytes; some lines shown as bytes; ";
    let n = u64::MAX;
    let cut = cut_facts(&Cut { line: n, from: n, to: n - 1, length: n });
    let longest_facts = format!("{encoding}{}lines {n}-{n} of {n} shown; {cut}", line_endings(u64::MAX, u64::MAX));
    let head = head(path, options, &longest_facts).len() + 1;
    // The note after the lines, at its longest.
    let rest = format!("=== line {n}: characters {n}-{n}, and lines {n}-{n} not shown ===\n").len();
    budget.saturating_sub(head + rest)
}

/// How the lines read end, when that is not plain LF: what a program that
/// writes the file back, or a model choosing how to change it, must keep.
fn line_endings(crlf: u64, lf: u64) -> String {
    match (crlf, lf) {
        (0, _) => String::new(),
        (_, 0) => "CRLF line endings; ".into(),
        (crlf, lf) => format!("mixed line endings, {crlf} CRLF and {lf} LF; "),
    }
}

/// The part of a line shown when the line does not fit, in characters as
/// shown, counting from 1.
pub(super) struct Cut {
    pub line: u64,
    pub from: u64,
    pub to: u64,
    pub length: u64,
}

pub(super) struct Read {
    pub body: String,
    first_shown: u64,
    pub shown_lines: u64,
    pub total_lines: u64,
    /// Characters of the last line shown.
    last_length: u64,
    pub cut: Option<Cut>,
    /// The budget ran out before the range did, at a line boundary.
    out_of_budget: bool,
    /// Lines read that end in CRLF, and in a bare LF.
    crlf: u64,
    lf: u64,
}

/// Keeps decoded lines inside the requested range and the budget, in bytes.
/// Lines are kept as shown, control characters written as `\xNN`, and
/// `--chars` counts those characters.
pub(super) struct Reader {
    budget: usize,
    range: Option<(u64, u64)>,
    chars: Option<(u64, u64)>,
    body: String,
    body_cost: usize,
    pending_note: String,
    /// The current line's selected characters, as shown, as many as could
    /// ever be shown of it.
    line: String,
    /// Characters of the current line, as shown, so far.
    position: u64,
    /// Characters of it kept in `line`.
    kept: u64,
    line_ends_with_cr: bool,
    number: u64,
    first_shown: u64,
    shown_lines: u64,
    last_length: u64,
    cut: Option<Cut>,
    out_of_budget: bool,
    crlf: u64,
    lf: u64,
    preview: bool,
}

impl Reader {
    pub(super) fn new(budget: usize, range: Option<(u64, u64)>, chars: Option<(u64, u64)>) -> Self {
        Self {
            budget,
            range,
            chars,
            body: String::new(),
            body_cost: 0,
            pending_note: String::new(),
            line: String::new(),
            position: 0,
            kept: 0,
            line_ends_with_cr: false,
            number: 1,
            first_shown: 0,
            shown_lines: 0,
            last_length: 0,
            cut: None,
            out_of_budget: false,
            crlf: 0,
            lf: 0,
            preview: false,
        }
    }

    pub(super) fn preview(budget: usize) -> Self {
        Self { preview: true, ..Self::new(budget, None, None) }
    }

    /// As `preview`, of the first `lines` lines only.
    pub(super) fn preview_lines(budget: usize, lines: u64) -> Self {
        Self { preview: true, ..Self::new(budget, Some((1, lines)), None) }
    }

    pub(super) fn take(&mut self, text: &str) {
        let mut text = text;
        if self.line_ends_with_cr {
            if let Some(rest) = text.strip_prefix('\n') {
                self.crlf += 1;
                self.end_line();
                text = rest;
            } else if !text.is_empty() {
                self.extend_line("\r");
                self.line_ends_with_cr = false;
            } else {
                return;
            }
        }
        for part in text.split_inclusive('\n') {
            if let Some(content) = part.strip_suffix('\n') {
                let (content, crlf) = content.strip_suffix('\r').map_or((content, false), |s| (s, true));
                self.extend_line(content);
                if crlf {
                    self.crlf += 1;
                } else {
                    self.lf += 1;
                }
                self.end_line();
            } else if let Some(content) = part.strip_suffix('\r') {
                self.extend_line(content);
                self.line_ends_with_cr = true;
            } else {
                self.extend_line(part);
            }
        }
    }

    pub(super) fn note(&mut self, note: &str) {
        self.pending_note = format!("{note}\n");
    }

    pub(super) fn prefix_filled(&self) -> bool {
        self.cut.is_some() || self.out_of_budget || self.line.len() > self.budget.saturating_sub(self.body_cost)
    }

    pub(super) fn cut_line(&self) -> Option<u64> {
        self.cut
            .as_ref()
            .map(|cut| cut.line)
            .or_else(|| (self.line.len() > self.budget.saturating_sub(self.body_cost)).then_some(self.number))
    }

    fn show_note(&mut self) {
        self.body_cost += self.pending_note.len();
        self.body.push_str(&self.pending_note);
        self.pending_note.clear();
    }

    fn wanted(&self) -> bool {
        let in_range = match self.range {
            Some((first, last)) => (first..=last).contains(&self.number),
            None => true,
        };
        in_range && self.cut.is_none() && !self.out_of_budget
    }

    fn extend_line(&mut self, content: &str) {
        if !self.wanted() {
            self.position += crate::text::visible_length(content) as u64;
            return;
        }
        // Keep no more than could ever be shown of this line.
        let room = self.budget.saturating_sub(self.body_cost);
        let (from, to) = self.chars.unwrap_or((1, u64::MAX));
        for (at, c) in content.char_indices() {
            if self.position >= to || self.line.len() > room {
                // What follows is only counted.
                self.position += crate::text::visible_length(&content[at..]) as u64;
                return;
            }
            let push = |shown: char, reader: &mut Self| {
                reader.position += 1;
                if (from..=to).contains(&reader.position) && reader.line.len() <= room {
                    reader.line.push(shown);
                    reader.kept += 1;
                }
            };
            if c.is_control() && c != '\t' {
                for byte in c.encode_utf8(&mut [0; 4]).bytes() {
                    for shown in format!(r"\x{byte:02X}").chars() {
                        push(shown, self);
                    }
                }
            } else {
                push(c, self);
            }
        }
    }

    /// The characters of the current line `--chars` selects.
    fn selected(&self) -> u64 {
        match self.chars {
            None => self.position,
            Some((from, to)) => self.position.min(to).saturating_sub(from - 1),
        }
    }

    fn end_line(&mut self) {
        if self.wanted() {
            let digits = self.number.to_string().len();
            let complete = self.kept == self.selected();
            let entry_cost = digits + 1 + self.line.len() + 1;
            let note_cost = self.pending_note.len();
            if complete && self.body_cost + note_cost + entry_cost <= self.budget {
                self.show_note();
                let line = std::mem::take(&mut self.line);
                self.show(&line);
            } else if (self.shown_lines == 0 || self.preview && self.position as usize > self.budget)
                && self.body_cost + note_cost + digits + 6 < self.budget
            {
                self.show_note();
                // A read ends inside a line only where that line is the first
                // or, for a preview, where it is longer than the whole preview
                // could ever be; otherwise it ends on a whole line.
                let room = self.budget - (self.body_cost + digits + 2);
                let shown = crate::answer::prefix(&self.line, room);
                let from = self.chars.map_or(1, |(from, _)| from);
                self.cut = Some(Cut {
                    line: self.number,
                    from,
                    to: from + shown.chars().count() as u64 - 1,
                    length: self.position,
                });
                self.body.push_str(&format!("{}\t{shown}\n", self.number));
            } else {
                self.out_of_budget = true;
            }
        }
        self.pending_note.clear();
        self.line.clear();
        self.position = 0;
        self.kept = 0;
        self.line_ends_with_cr = false;
        self.number += 1;
    }

    fn show(&mut self, content: &str) {
        if self.shown_lines == 0 {
            self.first_shown = self.number;
        }
        self.body.push_str(&format!("{}\t{content}\n", self.number));
        self.body_cost += self.number.to_string().len() + 1 + content.len() + 1;
        self.shown_lines += 1;
        self.last_length = self.position;
    }

    pub(super) fn finish(mut self) -> Read {
        if self.line_ends_with_cr {
            self.extend_line("\r");
            self.line_ends_with_cr = false;
        }
        // A last line without a newline is still a line.
        if self.position > 0 {
            self.end_line();
        }
        Read {
            body: self.body,
            first_shown: self.first_shown,
            shown_lines: self.shown_lines,
            total_lines: self.number - 1,
            last_length: self.last_length,
            cut: self.cut,
            out_of_budget: self.out_of_budget,
            crlf: self.crlf,
            lf: self.lf,
        }
    }
}
