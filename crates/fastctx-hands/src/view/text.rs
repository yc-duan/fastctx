//! Streaming file text after a shared encoding-evidence pass.
//!
//! At most one encoded line is decoded at a time: lines inside the
//! window and the budget are kept, and every other line is only counted, so
//! the head note can say `of N` for a file of any size. A line longer than
//! what is left of the budget is shown up to there, and the note says which
//! characters of it are shown.

use std::fs::{File, Metadata};
use std::io::{self, Seek as _};
use std::path::Path;

use fastctx_types::residents::Seen;
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};

use super::decode::Hashed;
use super::{Options, Section, head, hex, plural};
use crate::encoding::file::{Plan, visit};
pub(crate) use crate::text::visible;

pub fn look(
    path: &Path,
    options: &Options,
    budget: usize,
    mut file: File,
    start: Vec<u8>,
    metadata: &Metadata,
) -> io::Result<(Section, Option<Seen>)> {
    file.rewind()?;
    let (plan, evidence_hash) = {
        let mut scan = Hashed { reader: &mut file, hash: Sha256::new() };
        let plan = match Plan::inspect(&mut scan, options.encoding.as_deref()) {
            Ok(plan) => plan,
            Err(facts) => {
                let kind = kind(&start).map(|kind| format!("; {kind}")).unwrap_or_default();
                return Ok((Section::note(path, options, &format!("not shown: {facts}{kind}")), None));
            }
        };
        std::io::copy(&mut scan, &mut std::io::sink())?;
        (plan, scan.hash.finalize())
    };
    file.rewind()?;
    let mut stream = Hashed { reader: file, hash: Sha256::new() };
    let mut reader = Reader::new(budget_for_body(path, options, budget, &plan.facts()), options.lines);
    let mut labels = std::collections::BTreeSet::new();
    let mut readonly = false;
    let mut announced = String::new();
    if let Err(facts) = visit(&mut stream, &plan, |line| {
        let label = line.codec.map(|codec| codec.label());
        if let Some(label) = label {
            labels.insert(label);
        } else {
            readonly = true;
        }
        let key =
            format!("{:?}:{}:{:?}:{:?}", label, line.basis, line.issue, line.issue.as_ref().map(|_| line.bytes.start));
        if reader.wanted() && key != announced {
            let note = match &line.issue {
                Some(issue) => {
                    format!("[bytes {}..{}: read-only byte preview; {issue}]", line.bytes.start, line.bytes.end)
                }
                None => format!("[from byte {}: {}; {}]", line.bytes.start, label.expect("decoded line"), line.basis),
            };
            reader.note(&note);
            announced = key;
        }
        reader.take(&line.text);
        Ok(())
    }) {
        return Ok((Section::note(path, options, &format!("not shown: {facts}")), None));
    }
    let read_hash = stream.hash.finalize();
    if read_hash != evidence_hash {
        return Ok((
            Section::note(
                path,
                options,
                "not shown: file bytes changed between encoding selection and decoding; view the file again",
            ),
            None,
        ));
    }
    let read = reader.finish();

    let mut facts = plan.facts();
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
        // Selection was validated against every byte, including undisplayed lines.
        sha256: Some(hex(&read_hash)),
        size: metadata.len(),
        modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
        encoding: if readonly || labels.len() > 1 { None } else { labels.first().map(|label| (*label).into()) },
        encoding_selection: options.encoding.clone(),
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
    visible_chars: u64,
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
            visible_chars: 0,
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
                self.end_line(true);
            }
        }
    }

    fn note(&mut self, note: &str) {
        let cost = note.chars().count() + 1;
        if self.body_cost + cost <= self.budget {
            self.body.push_str(note);
            self.body.push('\n');
            self.body_cost += cost;
        } else {
            self.out_of_budget = true;
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
        self.visible_chars += crate::text::visible_length(content) as u64;
    }

    fn end_line(&mut self, newline: bool) {
        if self.wanted() {
            let content =
                visible(if newline { self.line.strip_suffix('\r').unwrap_or(&self.line) } else { &self.line });
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
                let total = self.visible_chars - 4 * u64::from(newline && self.line_ends_with_cr);
                self.body.push_str(&format!("{}\t{shown}\n", self.number));
                self.cut = Some(Cut { line: self.number, shown: shown.chars().count() as u64, length: total });
            } else {
                self.out_of_budget = true;
            }
        }
        self.line.clear();
        self.line_chars = 0;
        self.visible_chars = 0;
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
            self.end_line(false);
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
