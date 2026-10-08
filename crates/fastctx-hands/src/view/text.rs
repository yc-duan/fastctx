//! Showing a text file, read once from start to end.
//!
//! The file is decoded as it is read, never held whole: lines inside the
//! window and the budget are kept, and every other line is only counted, so
//! the head note can say `of N` for a file of any size. A line longer than
//! what is left of the budget is shown up to there, and the note says which
//! characters of it are shown.

use std::fs::{File, Metadata};
use std::io::{self, Read as _};
use std::path::Path;

use fastctx_types::residents::Seen;
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};

use super::decode::Decoder;
use super::{Options, Section, describe_error, head, hex, plural};

/// Bytes looked at to tell text from binary: a NUL among the first 8 KiB
/// means the file is not text in any encoding without a byte order mark.
pub(super) const SNIFF: usize = 8192;

pub fn look(
    path: &Path,
    options: &Options,
    budget: usize,
    mut file: File,
    mut start: Vec<u8>,
    metadata: &Metadata,
) -> io::Result<(Section, Option<Seen>)> {
    // Widen the first look to what the binary test needs.
    if start.len() < SNIFF {
        let mut more = vec![0u8; SNIFF - start.len()];
        let read = super::read_up_to(&mut file, &mut more)?;
        start.extend_from_slice(&more[..read]);
    }
    let mut decoder = match Decoder::new(&start, options.encoding.as_deref()) {
        Ok(chosen) => chosen,
        Err(facts) => return Ok((Section::note(path, options, &format!("not shown: {facts}")), None)),
    };
    if options.encoding.is_none() && matches!(decoder, Decoder::Auto(_)) && start.contains(&0) {
        let facts = format!("not shown: binary{}", kind(&start).map(|kind| format!(", {kind}")).unwrap_or_default());
        return Ok((Section::note(path, options, &facts), None));
    }
    let mut reader = Reader::new(budget_for_body(path, options, budget, &decoder.longest_facts()), options.lines);
    if let Err(facts) = decoder.feed(&start, false, &mut reader) {
        return Ok((Section::note(path, options, &format!("not shown: {facts}")), None));
    }
    let mut hasher = Sha256::new();
    hasher.update(&start);
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Ok((Section::note(path, options, &describe_error(&error)), None)),
        };
        hasher.update(&buffer[..read]);
        if let Err(facts) = decoder.feed(&buffer[..read], false, &mut reader) {
            return Ok((Section::note(path, options, &format!("not shown: {facts}")), None));
        }
    }
    if let Err(facts) = decoder.feed(&[], true, &mut reader) {
        return Ok((Section::note(path, options, &format!("not shown: {facts}")), None));
    }
    let read = reader.finish();

    let mut facts = decoder.facts();
    facts.push_str(&line_endings(read.crlf, read.lf));
    let total = read.total_lines;
    let whole = read.shown_lines == read.total_lines && read.cut.is_none();
    match (&read.cut, read.shown_lines) {
        (None, 0) if read.total_lines == 0 => facts.push_str("empty"),
        (None, _) if whole => facts.push_str(&plural(read.total_lines, "line", "lines")),
        (None, 0) => facts.push_str(&format!("no lines in the range asked; the file has {total} lines")),
        (None, shown) => {
            let first = read.first_shown;
            facts.push_str(&format!("lines {first}-{} of {total}", first + shown - 1));
        }
        (Some(cut), shown) => {
            let first = read.first_shown;
            if shown > 0 {
                facts.push_str(&format!("lines {first}-{} of {total} shown; ", first + shown - 1));
            }
            facts.push_str(&format!("line {}: characters 1-{} of {} shown", cut.line, cut.shown, cut.length));
        }
    }
    let mut text = format!("{}\n", head(path, options, &facts));
    text.push_str(&read.body);
    let cost = text.chars().count();
    let seen = Seen {
        path: path.to_string_lossy().into_owned(),
        at: Timestamp::now(),
        sha256: (whole && options.lines.is_none()).then(|| hex(&hasher.finalize())),
        size: metadata.len(),
        modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
        encoding: decoder.encoding(),
    };
    let shown = read.shown_lines > 0 || read.cut.is_some() || read.total_lines == 0;
    Ok((Section { text, cost, shown, partial: read.cut.is_some() || read.out_of_budget }, shown.then_some(seen)))
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

/// The budget left for the lines once the head note is paid for, the head
/// at its longest: the path, the machine, and numbers of up to twenty
/// digits in the longest form of the facts.
fn budget_for_body(path: &Path, options: &Options, budget: usize, encoding: &str) -> usize {
    let longest_facts = format!(
        "{encoding}{}lines {n}-{n} of {n} shown; line {n}: characters 1-{n} of {n} shown",
        line_endings(u64::MAX, u64::MAX),
        n = u64::MAX
    );
    let head = head(path, options, &longest_facts).chars().count() + 1;
    budget.saturating_sub(head)
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

struct Cut {
    line: u64,
    shown: u64,
    length: u64,
}

struct Read {
    body: String,
    first_shown: u64,
    shown_lines: u64,
    total_lines: u64,
    cut: Option<Cut>,
    /// The budget ran out before the range did, at a line boundary.
    out_of_budget: bool,
    /// Lines read that end in CRLF, and in a bare LF.
    crlf: u64,
    lf: u64,
}

/// Keeps decoded lines inside the requested range and character budget.
pub(super) struct Reader {
    budget: usize,
    range: Option<(u64, u64)>,
    body: String,
    body_cost: usize,
    line: String,
    line_chars: u64,
    line_ends_with_cr: bool,
    number: u64,
    first_shown: u64,
    shown_lines: u64,
    cut: Option<Cut>,
    out_of_budget: bool,
    crlf: u64,
    lf: u64,
}

impl Reader {
    fn new(budget: usize, range: Option<(u64, u64)>) -> Self {
        Self {
            budget,
            range,
            body: String::new(),
            body_cost: 0,
            line: String::new(),
            line_chars: 0,
            line_ends_with_cr: false,
            number: 1,
            first_shown: 0,
            shown_lines: 0,
            cut: None,
            out_of_budget: false,
            crlf: 0,
            lf: 0,
        }
    }

    pub(super) fn room(&self) -> usize {
        if self.wanted() { self.budget.saturating_sub(self.body_cost) } else { 0 }
    }

    pub(super) fn line(&mut self, text: String, characters: u64, ends_with_cr: bool, newline: bool) {
        self.line = text;
        self.line_chars = characters;
        self.line_ends_with_cr = ends_with_cr;
        if newline {
            self.count_ending();
        }
        self.end_line();
    }

    fn count_ending(&mut self) {
        if self.line_ends_with_cr { self.crlf += 1 } else { self.lf += 1 }
    }

    pub(super) fn take(&mut self, text: &str) {
        for part in text.split_inclusive('\n') {
            let (content, ends) = match part.strip_suffix('\n') {
                Some(content) => (content, true),
                None => (part, false),
            };
            self.extend_line(content);
            if ends {
                self.count_ending();
                self.end_line();
            }
        }
    }

    fn wanted(&self) -> bool {
        let in_range = match self.range {
            Some((first, last)) => (first..=last).contains(&self.number),
            None => true,
        };
        in_range && self.cut.is_none() && !self.out_of_budget
    }

    fn extend_line(&mut self, content: &str) {
        let chars = content.chars().count() as u64;
        if !content.is_empty() {
            self.line_ends_with_cr = content.ends_with('\r');
        }
        if self.wanted() {
            // Keep no more than could ever be shown of this line.
            let room = self.budget.saturating_sub(self.body_cost) as u64;
            let kept = self.line.chars().count() as u64;
            let take = room.saturating_add(1).saturating_sub(kept).min(chars);
            self.line.extend(content.chars().take(take as usize));
        }
        self.line_chars += chars;
    }

    fn end_line(&mut self) {
        if self.wanted() {
            let content = visible(self.line.strip_suffix('\r').unwrap_or(&self.line));
            let length = content.chars().count() as u64;
            let complete = self.line_chars == self.line.chars().count() as u64;
            let entry_cost = self.number.to_string().len() + 1 + content.chars().count() + 1;
            if complete && self.body_cost + entry_cost <= self.budget {
                self.show(&content, length as usize);
            } else if self.shown_lines == 0 {
                // Not even one whole line fits: show its start.
                let prefix = self.number.to_string().len() + 1 + 1;
                let room = self.budget.saturating_sub(self.body_cost + prefix);
                let shown: String = content.chars().take(room).collect();
                let total = self.line_chars - u64::from(self.line_ends_with_cr);
                self.body.push_str(&format!("{}\t{shown}\n", self.number));
                self.cut = Some(Cut { line: self.number, shown: shown.chars().count() as u64, length: total });
            } else {
                self.out_of_budget = true;
            }
        }
        self.line.clear();
        self.line_chars = 0;
        self.line_ends_with_cr = false;
        self.number += 1;
    }

    fn show(&mut self, content: &str, chars: usize) {
        if self.shown_lines == 0 {
            self.first_shown = self.number;
        }
        self.body.push_str(&format!("{}\t{content}\n", self.number));
        self.body_cost += self.number.to_string().len() + 1 + chars + 1;
        self.shown_lines += 1;
    }

    fn finish(mut self) -> Read {
        // A last line without a newline is still a line.
        if self.line_chars > 0 || !self.line.is_empty() {
            self.end_line();
        }
        Read {
            body: self.body,
            first_shown: self.first_shown,
            shown_lines: self.shown_lines,
            total_lines: self.number - 1,
            cut: self.cut,
            out_of_budget: self.out_of_budget,
            crlf: self.crlf,
            lf: self.lf,
        }
    }
}

/// A line's text with its control characters, tabs aside, written as
/// `\xNN` per byte: a terminal, or the `run` that reads this output as one,
/// would otherwise play them (a carriage return overwriting the line and its
/// number) instead of showing what the file holds.
pub(crate) fn visible(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() && c != '\t' {
            for byte in c.encode_utf8(&mut [0; 4]).bytes() {
                shown.push_str(&format!("\\x{byte:02X}"));
            }
        } else {
            shown.push(c);
        }
    }
    shown
}
