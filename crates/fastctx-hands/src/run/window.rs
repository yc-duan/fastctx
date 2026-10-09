//! The part of a job's output one answer shows.
//!
//! Files the resident asked to read with `fastctx view` are shown whole up
//! to the answer's budget (`world-view.md`, 一次给多少). Any other output is
//! shown whole when it fits the window a host's own shell gives; when it
//! does not, the answer shows a short start and end and names the lines in
//! between, which the resident reads back from the log with `fastctx view`.

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_types::run::{LegacyLines, OutputPiece, OutputWindow};

use super::terminal::{Line, Lines, Part, VIEW_MARK};

/// What the complete answer may hold, head notes and delivered words
/// included, in the units Codex counts a tool's output in: each text block
/// costs its UTF-8 bytes divided by four, rounded up (`world-view.md`).
pub const BUDGET: usize = 80_000;
/// What an image costs against the budget: the larger of the two hosts'
/// costs, Claude Code's for an image it scales to 2000x2000.
pub const IMAGE_COST: usize = 5_200;

/// What `text` costs as one block of an answer.
pub fn units(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// The UTF-8 bytes of text that cost at most `units`.
pub const fn bytes(units: usize) -> usize {
    units * 4
}

/// Raw image bytes one answer may carry. Claude Code runs on Node, where a
/// string holds at most about 5.4 × 10⁸ characters; the whole answer is one
/// JSON line and base64 makes images a third larger, so 400 MiB of images
/// is about the most an answer can carry and still be read (`world-view.md`).
pub const IMAGE_BYTES: usize = 400 * 1024 * 1024;

/// Characters of a program's output shown whole in one answer, an image
/// counted as `IMAGE_COST`: what Claude Code's own shell tool shows by
/// default, so either model reads it as it reads its host's shell.
pub const OUTPUT_WINDOW: usize = 30_000;

/// Characters of text shown of the start, and of the end, of a program's
/// output that does not fit its window. Such output is nearly always a long
/// log, read for how it began and how it ended; the lines between are named
/// with the command that reads them back.
pub const OUTPUT_HEAD: usize = 2_000;
pub const OUTPUT_TAIL: usize = 2_000;

/// Units `run`'s own notes can take beside a view's output: the first line,
/// a machine name of at most 32 characters with the facts of how the script
/// ended, the notes naming lines left out, and the note naming the log when
/// processes it started go on.
pub const RUN_NOTES: usize = 1_024;

/// The part of the output at `log` one answer shows: what `fastctx view`
/// wrote up to the whole budget, any other output in `OUTPUT_WINDOW`.
pub fn window(log: &Path) -> io::Result<OutputWindow> {
    snapshot(log, None)
}

/// Shows a retained prefix acknowledged by the output reader. Descendants
/// may keep appending while this answer is assembled.
pub fn window_through(log: &Path, bytes: u64) -> io::Result<OutputWindow> {
    snapshot(log, Some(bytes))
}

fn snapshot(log: &Path, through: Option<u64>) -> io::Result<OutputWindow> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(log)?;
    let available = file.metadata()?.len();
    let size = through.unwrap_or(available);
    if size > available {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the log is shorter than its retained output prefix"));
    }
    // Which regions `fastctx view` wrote and closed, read in a pass of its
    // own; output that never ran it, nearly all, skips that pass.
    let closed_views = if holds_view_mark((&mut file).take(size))? {
        file.seek(SeekFrom::Start(0))?;
        let mut scan = Lines::new((&mut file).take(size));
        while scan.next_line()?.is_some() {}
        scan.closed_views().clone()
    } else {
        Default::default()
    };
    file.seek(SeekFrom::Start(0))?;
    let mut regular = Selection::default();
    let mut lines = Lines::new((&mut file).take(size));
    let mut id = 0;
    while let Some(line) = lines.next_line()? {
        if !closed_views.contains(&line.view) {
            regular.push(Entry::of(id, &line));
        }
        id += 1;
    }
    let (chosen, used) = regular.finish();
    // A view's lines are shown whole while their bytes fit what is left.
    let mut view_left = bytes(BUDGET - RUN_NOTES).saturating_sub(used);
    file.seek(SeekFrom::Start(0))?;
    let mut lines = Lines::new(file.take(size));
    let mut assembly = Assembly::default();
    let mut omitted: Option<(u64, u64)> = None;
    id = 0;
    while let Some(line) = lines.next_line()? {
        let entry = Entry::of(id, &line);
        let portions = if closed_views.contains(&line.view) {
            let portions = if entry.bytes <= view_left {
                vec![Portion::Whole]
            } else {
                match chars_within(&line, view_left) {
                    0 => Vec::new(),
                    chars => vec![Portion::Start(chars)],
                }
            };
            view_left = view_left.saturating_sub(entry.bytes);
            portions
        } else {
            chosen.get(&id).cloned().unwrap_or_default()
        };
        if portions.is_empty() && entry.chars > 0 {
            let range = omitted.get_or_insert((line.number, line.through));
            range.1 = line.through;
        } else {
            if let Some((first, last)) = omitted.take() {
                assembly.omitted(first, last);
            }
            for portion in portions {
                match portion {
                    Portion::Whole => assembly.whole(&line),
                    Portion::Start(budget) => assembly.start_of(&line, budget),
                    Portion::End(budget) => assembly.end_of(&line, budget),
                }
            }
        }
        id += 1;
    }
    if let Some((first, last)) = omitted {
        assembly.omitted(first, last);
    }
    Ok(assembly.finish(log, size, lines.raw_lines()))
}

/// Whether the log holds the mark `fastctx view` writes first, read in
/// chunks that overlap by the mark's length less one.
fn holds_view_mark(mut file: impl io::Read) -> io::Result<bool> {
    let mark = &VIEW_MARK.as_bytes()[..VIEW_MARK.len() - 1];
    let finder = memchr::memmem::Finder::new(mark);
    let mut buffer = vec![0u8; 1 << 20];
    let mut kept = 0;
    loop {
        let read = file.read(&mut buffer[kept..])?;
        if read == 0 {
            return Ok(false);
        }
        let filled = kept + read;
        if finder.find(&buffer[..filled]).is_some() {
            return Ok(true);
        }
        kept = (mark.len() - 1).min(filled);
        buffer.copy_within(filled - kept..filled, 0);
    }
}

#[derive(Clone)]
enum Portion {
    Whole,
    /// The first characters of a line.
    Start(usize),
    /// The last characters of a line.
    End(usize),
}

/// One line of the output as the selection weighs it.
#[derive(Clone, Copy)]
struct Entry {
    id: u64,
    /// Against the window: characters, each image at `IMAGE_COST`.
    chars: usize,
    /// Against the start and end of output that does not fit: characters of
    /// text alone, so an image a program drew there is still shown.
    text: usize,
    /// Against the answer: bytes of text, each image at `bytes(IMAGE_COST)`.
    bytes: usize,
}

impl Entry {
    fn of(id: u64, line: &Line) -> Self {
        let newline = usize::from(line.newline);
        let mut entry = Self { id, chars: newline, text: newline, bytes: newline };
        for part in &line.parts {
            let (chars, text, size) = match part {
                Part::Text(text) => (text.chars().count(), text.chars().count(), text.len()),
                Part::Spaces(count) => (*count, *count, *count),
                Part::Image(_) => (IMAGE_COST, 0, bytes(IMAGE_COST)),
                Part::Unreadable(reason) => (reason.chars().count(), reason.chars().count(), reason.len()),
            };
            entry.chars = entry.chars.saturating_add(chars);
            entry.text = entry.text.saturating_add(text);
            entry.bytes = entry.bytes.saturating_add(size);
        }
        entry
    }
}

/// Chooses what of a program's output to show: all of it when it fits the
/// window, otherwise whole lines of the start and of the end within
/// `OUTPUT_HEAD` and `OUTPUT_TAIL`, a single line longer than that shown in
/// part. Only positions and costs are kept while scanning, so large lines
/// and image payloads need not stay in memory.
#[derive(Default)]
struct Selection {
    /// Every line so far, while all of them fit the window.
    lines: Vec<Entry>,
    chars: usize,
    overflowed: bool,
    head: Vec<Entry>,
    /// A first line longer than the start, shown in part.
    first_cut: Option<Entry>,
    tail: VecDeque<Entry>,
    tail_text: usize,
}

impl Selection {
    fn push(&mut self, entry: Entry) {
        if entry.chars == 0 {
            return;
        }
        if !self.overflowed {
            if entry.chars <= OUTPUT_WINDOW - self.chars {
                self.chars += entry.chars;
                self.lines.push(entry);
                return;
            }
            self.overflowed = true;
            let mut earlier = std::mem::take(&mut self.lines).into_iter().chain([entry]).peekable();
            let mut head_text = 0;
            while let Some(next) = earlier.next_if(|next| next.text <= OUTPUT_HEAD - head_text) {
                head_text += next.text;
                self.head.push(next);
            }
            if self.head.is_empty() {
                // The first line alone is longer than the start: shown in
                // part, and still the end when nothing follows it.
                self.first_cut = earlier.peek().copied();
            }
            for next in earlier {
                self.push_tail(next);
            }
            return;
        }
        self.push_tail(entry);
    }

    /// The end keeps the last whole lines within `OUTPUT_TAIL`, or the end
    /// of the last line when it alone is longer.
    fn push_tail(&mut self, entry: Entry) {
        self.tail_text = self.tail_text.saturating_add(entry.text);
        self.tail.push_back(entry);
        while self.tail.len() > 1 && self.tail_text > OUTPUT_TAIL {
            self.tail_text -= self.tail.pop_front().expect("nonempty").text;
        }
    }

    /// The portions shown of each chosen line, and at most how many bytes
    /// of the answer they take.
    fn finish(self) -> (std::collections::BTreeMap<u64, Vec<Portion>>, usize) {
        let mut chosen = std::collections::BTreeMap::<u64, Vec<Portion>>::new();
        let mut used = 0usize;
        let mut show = |id: u64, portion: Portion, size: usize| {
            chosen.entry(id).or_default().push(portion);
            used = used.saturating_add(size);
        };
        if !self.overflowed {
            for entry in &self.lines {
                show(entry.id, Portion::Whole, entry.bytes);
            }
            return (chosen, used);
        }
        for entry in &self.head {
            show(entry.id, Portion::Whole, entry.bytes);
        }
        let lone = (self.tail.len() == 1 && self.tail_text > OUTPUT_TAIL).then(|| self.tail[0]);
        match (self.first_cut, lone) {
            // A single line longer than the window, though not so long that
            // its start and end would leave anything out: shown whole.
            (Some(first), Some(last)) if first.id == last.id && first.text <= OUTPUT_HEAD + OUTPUT_TAIL => {
                show(first.id, Portion::Whole, first.bytes);
                return (chosen, used);
            }
            (Some(first), _) => show(first.id, Portion::Start(OUTPUT_HEAD), bytes(OUTPUT_HEAD)),
            (None, _) => {}
        }
        match lone {
            Some(last) => show(last.id, Portion::End(OUTPUT_TAIL), bytes(OUTPUT_TAIL)),
            None => {
                for entry in &self.tail {
                    // A first line cut at its start is not shown again whole.
                    if self.first_cut.is_none_or(|first| first.id != entry.id) {
                        show(entry.id, Portion::Whole, entry.bytes);
                    }
                }
            }
        }
        (chosen, used)
    }
}

/// How many characters of a line's text fit in `room` bytes.
fn chars_within(line: &Line, mut room: usize) -> usize {
    let mut chars = 0;
    for part in &line.parts {
        match part {
            Part::Text(text) => {
                for c in text.chars() {
                    if c.len_utf8() > room {
                        return chars;
                    }
                    room -= c.len_utf8();
                    chars += 1;
                }
            }
            Part::Spaces(count) => {
                let fitting = (*count).min(room);
                chars += fitting;
                room -= fitting;
                if fitting < *count {
                    return chars;
                }
            }
            _ => {}
        }
    }
    chars
}

#[derive(Default)]
struct Assembly {
    pieces: Vec<OutputPiece>,
    image_bytes: usize,
    legacy: std::collections::BTreeMap<String, std::collections::BTreeSet<u64>>,
    escaped: std::collections::BTreeSet<u64>,
}

impl Assembly {
    fn whole(&mut self, line: &Line) {
        self.count(line);
        for part in &line.parts {
            match part {
                Part::Text(text) => self.text(line.number, text),
                Part::Spaces(count) => self.text(line.number, &" ".repeat(*count)),
                Part::Image(image) => {
                    if self.image_bytes + image.bytes.len() > IMAGE_BYTES {
                        self.pieces.push(OutputPiece::Unreadable {
                            line: line.number,
                            reason: format!(
                                "an image of {} bytes that would take this answer past {IMAGE_BYTES} bytes of images",
                                image.bytes.len()
                            ),
                        });
                    } else {
                        self.image_bytes += image.bytes.len();
                        self.pieces.push(OutputPiece::Image {
                            mime: image.mime.to_string(),
                            width: image.width,
                            height: image.height,
                            base64: STANDARD.encode(&image.bytes),
                        });
                    }
                }
                Part::Unreadable(reason) => {
                    self.pieces.push(OutputPiece::Unreadable { line: line.number, reason: reason.clone() })
                }
            }
        }
        if line.newline {
            self.text(line.number, "\n");
        }
    }

    /// The first `budget` characters of a line too long to show whole.
    fn start_of(&mut self, line: &Line, budget: usize) {
        self.count(line);
        self.images_left_out(line);
        let of = text_cost(line);
        if of == 0 {
            return;
        }
        let shown = line_slice(line, 0, budget);
        let to = shown.chars().count();
        self.pieces.push(OutputPiece::PartOfLine {
            line: line.number,
            from: line.offset as u64 + 1,
            to: (line.offset + to) as u64,
            of: line.characters as u64,
            text: shown,
        });
    }

    /// The last `budget` characters of a line too long to show whole.
    fn end_of(&mut self, line: &Line, budget: usize) {
        self.count(line);
        self.images_left_out(line);
        let of = text_cost(line);
        if of == 0 {
            return;
        }
        let skip = of.saturating_sub(budget);
        let shown = line_slice(line, skip, budget);
        self.pieces.push(OutputPiece::PartOfLine {
            line: line.number,
            from: (line.offset + skip) as u64 + 1,
            to: (line.offset + of) as u64,
            of: line.characters as u64,
            text: shown,
        });
    }

    /// A line shown in part is shown as text; images in it are named.
    fn images_left_out(&mut self, line: &Line) {
        for part in &line.parts {
            if let Part::Unreadable(reason) = part {
                self.pieces.push(OutputPiece::Unreadable { line: line.number, reason: reason.clone() });
            }
        }
        let images = line.parts.iter().filter(|part| matches!(part, Part::Image(_))).count();
        if images > 0 {
            self.pieces.push(OutputPiece::Unreadable {
                line: line.number,
                reason: format!("{images} image(s) in a line too long to show whole; the log holds them"),
            });
        }
    }

    fn omitted(&mut self, first_line: u64, last_line: u64) {
        self.pieces.push(OutputPiece::Omitted { first_line, last_line });
    }

    fn text(&mut self, line: u64, text: &str) {
        if let Some(OutputPiece::Text { text: open, .. }) = self.pieces.last_mut() {
            open.push_str(text);
        } else {
            self.pieces.push(OutputPiece::Text { first_line: line, text: text.to_string() });
        }
    }

    fn count(&mut self, line: &Line) {
        if line.decoding.escaped {
            self.escaped.insert(line.number);
        }
        for name in &line.decoding.legacy {
            self.legacy.entry(name.clone()).or_default().insert(line.number);
        }
    }

    fn finish(mut self, log: &Path, bytes: u64, lines: u64) -> OutputWindow {
        self.pieces.retain(|piece| !matches!(piece, OutputPiece::Text { text, .. } if text.is_empty()));
        OutputWindow {
            log: log.to_string_lossy().into_owned(),
            bytes,
            lines,
            pieces: self.pieces,
            legacy: self
                .legacy
                .into_iter()
                .map(|(encoding, lines)| LegacyLines { encoding, lines: lines.len() as u64 })
                .collect(),
            escaped_lines: self.escaped.len() as u64,
        }
    }
}

fn text_cost(line: &Line) -> usize {
    line.parts
        .iter()
        .map(|part| match part {
            Part::Text(text) => text.chars().count(),
            Part::Spaces(count) => *count,
            _ => 0,
        })
        .fold(0usize, usize::saturating_add)
}

/// Copies only the requested characters, without expanding sparse gaps.
fn line_slice(line: &Line, mut skip: usize, mut take: usize) -> String {
    let mut out = String::new();
    for part in &line.parts {
        let size = match part {
            Part::Text(text) => text.chars().count(),
            Part::Spaces(count) => *count,
            _ => continue,
        };
        if skip >= size {
            skip -= size;
            continue;
        }
        let count = take.min(size - skip);
        match part {
            Part::Text(text) => out.extend(text.chars().skip(skip).take(count)),
            Part::Spaces(_) => out.extend(std::iter::repeat_n(' ', count)),
            _ => unreachable!(),
        }
        take -= count;
        skip = 0;
        if take == 0 {
            break;
        }
    }
    out
}
