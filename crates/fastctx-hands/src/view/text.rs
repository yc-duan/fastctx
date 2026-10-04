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

use encoding_rs::{DecoderResult, Encoding, UTF_8, UTF_16BE, UTF_16LE};
use fastctx_types::residents::Seen;
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};

use super::{Options, Section, describe_error, head, hex, plural};

/// Bytes looked at to tell text from binary: a NUL among the first 8 KiB
/// means the file is not text in any encoding without a byte order mark.
const SNIFF: usize = 8192;

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
    let (encoding, bom) = match choose_encoding(&start, options) {
        Ok(chosen) => chosen,
        Err(facts) => return Ok((Section::note(path, options, &facts), None)),
    };
    let mut reader = Reader::new(encoding, bom, budget_for_body(path, options, budget), options.lines);
    reader.feed(&start[bom..], false);
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
        reader.feed(&buffer[..read], false);
    }
    reader.feed(&[], true);
    let read = reader.finish();

    let encoding_fact = match (encoding == UTF_8, bom > 0) {
        (true, false) => String::new(),
        (true, true) => "UTF-8 with BOM; ".to_string(),
        (false, _) => format!("decoded from {}; ", encoding.name()),
    };
    let mut facts = encoding_fact;
    let total = if read.total_known { read.total_lines.to_string() } else { format!("at least {}", read.total_lines) };
    let whole = read.total_known && read.shown_lines == read.total_lines && read.cut.is_none() && read.total_lines > 0;
    match (&read.cut, read.shown_lines) {
        (None, 0) if read.total_lines == 0 && read.total_known => facts.push_str("empty"),
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
    if let Some(at) = read.malformed_at {
        let note = format!(
            "; not valid {} from byte {at}, which is not shown; name its encoding with --encoding",
            encoding.name()
        );
        facts.push_str(&note);
    }
    let mut text = format!("{}\n", head(path, options, &facts));
    text.push_str(&read.body);
    let cost = text.chars().count();
    let seen = Seen {
        path: path.to_string_lossy().into_owned(),
        at: Timestamp::now(),
        sha256: (whole && read.malformed_at.is_none() && options.lines.is_none()).then(|| hex(&hasher.finalize())),
        size: metadata.len(),
        modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
        encoding: Some(encoding.name().to_string()),
    };
    let shown = read.shown_lines > 0 || read.cut.is_some();
    Ok((Section { text, cost, shown, partial: read.cut.is_some() || read.out_of_budget }, shown.then_some(seen)))
}

/// The encoding to read with, and how many bytes of byte order mark to
/// skip; or, when the file is not text that can be read without being
/// told its encoding, the facts to say instead.
fn choose_encoding(start: &[u8], options: &Options) -> Result<(&'static Encoding, usize), String> {
    if let Some(label) = &options.encoding {
        let encoding = Encoding::for_label_no_replacement(label.trim().as_bytes())
            .ok_or_else(|| format!("not shown: {label:?} names no encoding this build knows"))?;
        let bom = match Encoding::for_bom(start) {
            Some((found, length)) if found == encoding => length,
            _ => 0,
        };
        return Ok((encoding, bom));
    }
    if start.starts_with(&[0xff, 0xfe, 0, 0]) || start.starts_with(&[0, 0, 0xfe, 0xff]) {
        return Err("not shown: UTF-32 text, which this build does not read".to_string());
    }
    if let Some((encoding, length)) = Encoding::for_bom(start) {
        return Ok((encoding, length));
    }
    if start.contains(&0) {
        return Err(format!("not shown: binary{}", kind(start).map(|kind| format!(", {kind}")).unwrap_or_default()));
    }
    if std::str::from_utf8(start).is_err_and(|error| error.error_len().is_some()) {
        return Err(
            "not shown: not UTF-8, and its encoding is not named; if it is text, name its encoding with --encoding, such as --encoding gbk"
                .to_string(),
        );
    }
    Ok((UTF_8, 0))
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
fn budget_for_body(path: &Path, options: &Options, budget: usize) -> usize {
    let longest_facts = format!(
        "decoded from {}; lines {n}-{n} of {n} shown; line {n}: characters 1-{n} of {n} shown; not valid {} from byte {n}, which is not shown; name its encoding with --encoding",
        "x-mac-cyrillic",
        "x-mac-cyrillic",
        n = u64::MAX
    );
    let head = head(path, options, &longest_facts).chars().count() + 1;
    budget.saturating_sub(head)
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
    /// False when undecodable bytes in an encoding whose newlines are not
    /// plain bytes leave the count a lower bound.
    total_known: bool,
    cut: Option<Cut>,
    /// The budget ran out before the range did, at a line boundary.
    out_of_budget: bool,
    malformed_at: Option<u64>,
}

/// Decodes as bytes arrive, keeping the lines in the window.
struct Reader {
    decoder: encoding_rs::Decoder,
    ascii_compatible: bool,
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
    malformed_at: Option<u64>,
    consumed: u64,
    /// After a malformed sequence, lines are counted from the raw bytes.
    raw_newlines: u64,
    saw_bytes_after_last_newline: bool,
}

impl Reader {
    /// `skipped` bytes, a byte order mark, come before what is fed.
    fn new(encoding: &'static Encoding, skipped: usize, budget: usize, range: Option<(u64, u64)>) -> Self {
        Self {
            decoder: encoding.new_decoder_without_bom_handling(),
            ascii_compatible: encoding != UTF_16LE && encoding != UTF_16BE,
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
            malformed_at: None,
            consumed: skipped as u64,
            raw_newlines: 0,
            saw_bytes_after_last_newline: false,
        }
    }

    fn feed(&mut self, bytes: &[u8], last: bool) {
        if self.malformed_at.is_some() {
            self.count_raw(bytes);
            return;
        }
        let mut input = bytes;
        let mut piece = String::with_capacity(input.len() + 16);
        loop {
            piece.clear();
            piece.reserve(self.decoder.max_utf8_buffer_length_without_replacement(input.len()).unwrap_or(input.len() * 3));
            let (result, read) = self.decoder.decode_to_string_without_replacement(input, &mut piece, last);
            self.take(&piece);
            match result {
                DecoderResult::InputEmpty => {
                    self.consumed += read as u64;
                    return;
                }
                DecoderResult::OutputFull => {
                    self.consumed += read as u64;
                    input = &input[read..];
                }
                DecoderResult::Malformed(bad, after) => {
                    let at = self.consumed + read as u64 - u64::from(bad) - u64::from(after);
                    self.malformed_at = Some(at);
                    // The bad bytes belong to the line they are in; the bytes
                    // read past them are counted with the rest.
                    self.saw_bytes_after_last_newline = true;
                    self.count_raw(&input[read - usize::from(after)..]);
                    return;
                }
            }
        }
    }

    /// Counts lines in bytes that will not be decoded, by their newlines,
    /// as every ASCII-compatible encoding writes them.
    fn count_raw(&mut self, bytes: &[u8]) {
        if !self.ascii_compatible || bytes.is_empty() {
            return;
        }
        self.raw_newlines += bytes.iter().filter(|&&b| b == b'\n').count() as u64;
        self.saw_bytes_after_last_newline = bytes.last() != Some(&b'\n');
    }

    fn take(&mut self, text: &str) {
        for part in text.split_inclusive('\n') {
            let (content, ends) = match part.strip_suffix('\n') {
                Some(content) => (content, true),
                None => (part, false),
            };
            self.extend_line(content);
            if ends {
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
            let content = self.line.strip_suffix('\r').unwrap_or(&self.line).to_string();
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
        if self.malformed_at.is_none() && (self.line_chars > 0 || !self.line.is_empty()) {
            self.end_line();
        }
        let mut total = self.number - 1;
        if self.malformed_at.is_some() {
            total = self.number - 1 + self.raw_newlines + u64::from(self.saw_bytes_after_last_newline);
        }
        Read {
            body: self.body,
            first_shown: self.first_shown,
            shown_lines: self.shown_lines,
            total_lines: total,
            total_known: self.malformed_at.is_none() || self.ascii_compatible,
            cut: self.cut,
            out_of_budget: self.out_of_budget,
            malformed_at: self.malformed_at,
        }
    }
}
