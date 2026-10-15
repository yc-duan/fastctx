//! The part of a job's output one answer shows.
//!
//! Files the resident asked to read with `fastctx view` are shown whole up
//! to the answer's budget (`world-view.md`, 一次给多少). Any other output is
//! shown whole when it fits the window a host's own shell gives; when it
//! does not, the answer shows a short start and end and names the lines in
//! between. The log retains every original byte.

use super::queue::Queue;
use std::fs::File;
use std::io;
use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use fastctx_types::run::{LegacyLines, OutputPiece, OutputWindow};

use super::terminal::{Line, Lines, Part, VIEW_MARK};

/// What the complete answer may hold, head notes and delivered words
/// included, in the units Codex counts a tool's output in: each text block
/// costs its UTF-8 bytes divided by four, rounded up (`world-view.md`).
pub const BUDGET: usize = 50_000;
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

/// Units of a program's output shown whole in one answer, an image counted
/// as `IMAGE_COST`: between what the hosts' own shells show (Claude Code's
/// 30,000 characters, Codex's 10,000 tokens), and the same window a file
/// gets before its content is known (`world-run.md`, 输出).
pub const OUTPUT_WINDOW: usize = 10_000;

/// Units of text shown of the start, and of the end, of a program's output
/// that does not fit its window. Such output is nearly always a long log,
/// read for how it began and how it ended; the answer identifies the
/// omitted lines and the file holding the complete output.
pub const OUTPUT_HEAD: usize = 2_000;
pub const OUTPUT_TAIL: usize = 1_000;

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
        if !closed_views.contains(&line.view)? {
            regular.push(Entry::of(id, &line)?)?;
        }
        id += 1;
    }
    let (mut chosen, used) = regular.finish()?;
    // A view's lines are shown whole while their bytes fit what is left.
    let mut view_left = bytes(BUDGET - RUN_NOTES).saturating_sub(used);
    file.seek(SeekFrom::Start(0))?;
    let mut lines = Lines::new(file.take(size));
    let mut assembly = Assembly::default();
    let mut omitted: Option<(u64, u64)> = None;
    id = 0;
    while let Some(line) = lines.next_line()? {
        let entry = Entry::of(id, &line)?;
        let portions = if closed_views.contains(&line.view)? {
            let portions = if entry.bytes <= view_left {
                vec![Portion::Whole]
            } else {
                match chars_within(&line, view_left)? {
                    0 => Vec::new(),
                    chars => vec![Portion::Start(chars)],
                }
            };
            view_left = view_left.saturating_sub(entry.bytes);
            portions
        } else {
            // The selection measured the start and the end of a line in
            // bytes; the line itself says how many characters fit them.
            let mut portions = Vec::new();
            for portion in chosen.get(id)? {
                portions.push(match portion {
                    Portion::Start(room) => Portion::Start(chars_within(&line, room)?),
                    Portion::End(room) => Portion::End(chars_within_end(&line, room)?),
                    Portion::Whole => Portion::Whole,
                });
            }
            portions
        };
        if portions.is_empty() && entry.bytes > 0 {
            let range = omitted.get_or_insert((line.number, line.through));
            range.1 = line.through;
        } else {
            if let Some((first, last)) = omitted.take() {
                assembly.omitted(first, last);
            }
            for portion in portions {
                match portion {
                    Portion::Whole => assembly.whole(&line)?,
                    Portion::Start(budget) => assembly.start_of(&line, budget)?,
                    Portion::End(budget) => assembly.end_of(&line, budget)?,
                }
            }
        }
        id += 1;
    }
    if let Some((first, last)) = omitted {
        assembly.omitted(first, last);
    }
    if let Some(unfinished) = lines.unfinished() {
        assembly.pieces.push(OutputPiece::Unreadable {
            line: unfinished.line,
            reason: format!("{}; raw bytes: {}", unfinished.description(), log.display()),
        });
    }
    let mut window = assembly.finish(log, size, lines.raw_lines());
    window.failed = lines.failed().to_vec();
    Ok(window)
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

/// What of a line an answer shows. In the selection, a start or an end is
/// the bytes of text it may take; once the line is read again, the
/// characters that fit them.
#[derive(Clone)]
enum Portion {
    Whole,
    /// The first characters of a line.
    Start(usize),
    /// The last characters of a line.
    End(usize),
}

/// One line of the output as the selection weighs it, in bytes, which the
/// units are counted from (`units`).
#[derive(Clone, Copy)]
struct Entry {
    id: u64,
    /// Against the start and end of output that does not fit: bytes of text
    /// alone, so an image a program drew there is still shown.
    text: usize,
    /// Against the window and the answer: bytes of text, each image at
    /// `bytes(IMAGE_COST)`.
    bytes: usize,
}

impl Entry {
    fn of(id: u64, line: &Line) -> io::Result<Self> {
        let newline = usize::from(line.newline);
        if let Some(text) = line.parts.text() {
            let size = text.len() + newline;
            return Ok(Self { id, text: size, bytes: size });
        }
        let mut entry = Self { id, text: newline, bytes: newline };
        for part in line.parts.iter() {
            let part = part?;
            let (text, size) = match &part {
                Part::Text(text) => (text.len(), text.len()),
                Part::Spaces(count) => (*count, *count),
                Part::Image(_) => (0, bytes(IMAGE_COST)),
                Part::Unreadable(reason) => (reason.len(), reason.len()),
            };
            entry.text = entry.text.saturating_add(text);
            entry.bytes = entry.bytes.saturating_add(size);
        }
        Ok(entry)
    }
}

impl Entry {
    fn record(self) -> [u64; 3] {
        [self.id, self.text as u64, self.bytes as u64]
    }
    fn read(record: [u64; 3]) -> Self {
        Self { id: record[0], text: record[1] as usize, bytes: record[2] as usize }
    }
}

#[derive(Default)]
struct Selection {
    lines: Queue<3>,
    bytes: usize,
    overflowed: bool,
    head: Queue<3>,
    first_cut: Option<Entry>,
    tail: Queue<3>,
    tail_text: usize,
}
impl Selection {
    fn push(&mut self, entry: Entry) -> io::Result<()> {
        if entry.bytes == 0 {
            return Ok(());
        }
        if !self.overflowed {
            if entry.bytes <= bytes(OUTPUT_WINDOW) - self.bytes {
                self.bytes += entry.bytes;
                return self.lines.push(entry.record());
            }
            self.overflowed = true;
            let mut earlier = std::mem::take(&mut self.lines);
            earlier.push(entry.record())?;
            let mut head_text = 0;
            while let Some(next) = earlier.front()?.map(Entry::read) {
                if next.text > bytes(OUTPUT_HEAD) - head_text {
                    break;
                }
                head_text += next.text;
                self.head.push(earlier.pop()?.expect("front"))?;
            }
            if self.head.is_empty() {
                self.first_cut = earlier.front()?.map(Entry::read);
            }
            while let Some(next) = earlier.pop()? {
                self.push_tail(Entry::read(next))?;
            }
            return Ok(());
        }
        self.push_tail(entry)
    }
    fn push_tail(&mut self, entry: Entry) -> io::Result<()> {
        self.tail_text = self.tail_text.saturating_add(entry.text);
        self.tail.push(entry.record())?;
        while self.tail.len() > 1 && self.tail_text > bytes(OUTPUT_TAIL) {
            self.tail_text -= Entry::read(self.tail.pop()?.expect("nonempty")).text;
        }
        Ok(())
    }
    fn finish(mut self) -> io::Result<(Chosen, usize)> {
        let mut chosen = Chosen::default();
        let mut used = 0usize;
        let mut show = |id: u64, portion: Portion, size: usize| -> io::Result<()> {
            chosen.push(id, portion)?;
            used = used.saturating_add(size);
            Ok(())
        };
        if !self.overflowed {
            while let Some(entry) = self.lines.pop()?.map(Entry::read) {
                show(entry.id, Portion::Whole, entry.bytes)?;
            }
            return Ok((chosen, used));
        }
        while let Some(entry) = self.head.pop()?.map(Entry::read) {
            show(entry.id, Portion::Whole, entry.bytes)?;
        }
        let lone = if self.tail.len() == 1 && self.tail_text > bytes(OUTPUT_TAIL) {
            self.tail.front()?.map(Entry::read)
        } else {
            None
        };
        match (self.first_cut, lone) {
            (Some(first), Some(last)) if first.id == last.id && first.text <= bytes(OUTPUT_HEAD + OUTPUT_TAIL) => {
                show(first.id, Portion::Whole, first.bytes)?;
                return Ok((chosen, used));
            }
            (Some(first), _) => show(first.id, Portion::Start(bytes(OUTPUT_HEAD)), bytes(OUTPUT_HEAD))?,
            (None, _) => {}
        }
        match lone {
            Some(last) => show(last.id, Portion::End(bytes(OUTPUT_TAIL)), bytes(OUTPUT_TAIL))?,
            None => {
                while let Some(entry) = self.tail.pop()?.map(Entry::read) {
                    if self.first_cut.is_none_or(|first| first.id != entry.id) {
                        show(entry.id, Portion::Whole, entry.bytes)?;
                    }
                }
            }
        }
        Ok((chosen, used))
    }
}
#[derive(Default)]
struct Chosen(Queue<3>);
impl Chosen {
    fn push(&mut self, id: u64, portion: Portion) -> io::Result<()> {
        let (kind, count) = match portion {
            Portion::Whole => (0, 0),
            Portion::Start(n) => (1, n),
            Portion::End(n) => (2, n),
        };
        self.0.push([id, kind, count as u64])
    }
    fn get(&mut self, id: u64) -> io::Result<Vec<Portion>> {
        let mut portions = Vec::new();
        while let Some([next, kind, count]) = self.0.front()? {
            if next > id {
                break;
            }
            self.0.pop()?;
            if next == id {
                portions.push(match kind {
                    0 => Portion::Whole,
                    1 => Portion::Start(count as usize),
                    2 => Portion::End(count as usize),
                    _ => unreachable!("written kind"),
                });
            }
        }
        Ok(portions)
    }
}

/// How many characters of a line's text fit in `room` bytes.
fn chars_within(line: &Line, mut room: usize) -> io::Result<usize> {
    let mut chars = 0;
    for part in line.parts.iter() {
        let part = part?;
        match &part {
            Part::Text(text) => {
                for c in text.chars() {
                    if c.len_utf8() > room {
                        return Ok(chars);
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
                    return Ok(chars);
                }
            }
            _ => {}
        }
    }
    Ok(chars)
}

/// How many of a line's last characters of text fit in `room` bytes.
fn chars_within_end(line: &Line, room: usize) -> io::Result<usize> {
    let mut total = 0usize;
    for part in line.parts.iter() {
        total = total.saturating_add(match part? {
            Part::Text(text) => text.len(),
            Part::Spaces(count) => count,
            _ => 0,
        });
    }
    // Read from the start: what lies before the last `room` bytes is passed
    // over, a character across that boundary with it.
    let mut skip = total.saturating_sub(room);
    let mut chars = 0;
    for part in line.parts.iter() {
        match part? {
            Part::Text(text) => {
                for c in text.chars() {
                    if skip > 0 {
                        skip = skip.saturating_sub(c.len_utf8());
                    } else {
                        chars += 1;
                    }
                }
            }
            Part::Spaces(count) => {
                let passed = count.min(skip);
                skip -= passed;
                chars += count - passed;
            }
            _ => {}
        }
    }
    Ok(chars)
}

#[derive(Default)]
struct Assembly {
    pieces: super::pieces::Pieces,
    // Include images discarded by final-answer edge retention: refunding them
    // would change which later images exceed the existing raw-image allowance.
    image_bytes: usize,
    legacy: std::collections::BTreeMap<String, LineCount>,
    escaped: LineCount,
    graphics_noted: Option<u64>,
}

impl Assembly {
    fn whole(&mut self, line: &Line) -> io::Result<()> {
        self.count(line);
        if let Some(text) = line.parts.text() {
            self.text(line.number, text);
            if line.newline {
                self.text(line.number, "\n");
            }
            return Ok(());
        }
        for part in line.parts.iter() {
            let part = part?;
            match &part {
                Part::Text(text) => self.text(line.number, text),
                Part::Spaces(count) => self.text(line.number, &" ".repeat(*count)),
                Part::Image(image) => {
                    if image.bytes.len() > IMAGE_BYTES - self.image_bytes {
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
                            base64: encode_image(&image.bytes)?,
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
        Ok(())
    }

    /// The first `budget` characters of a line too long to show whole.
    fn start_of(&mut self, line: &Line, budget: usize) -> io::Result<()> {
        self.count(line);
        self.images_left_out(line)?;
        let of = text_cost(line)?;
        if of == 0 {
            return Ok(());
        }
        let shown = line_slice(line, 0, budget)?;
        let to = shown.chars().count();
        self.pieces.push(OutputPiece::PartOfLine {
            line: line.number,
            from: line.offset as u64 + 1,
            to: (line.offset + to) as u64,
            of: line.characters as u64,
            text: shown,
        });
        Ok(())
    }

    /// The last `budget` characters of a line too long to show whole.
    fn end_of(&mut self, line: &Line, budget: usize) -> io::Result<()> {
        self.count(line);
        self.images_left_out(line)?;
        let of = text_cost(line)?;
        if of == 0 {
            return Ok(());
        }
        let skip = of.saturating_sub(budget);
        let shown = line_slice(line, skip, budget)?;
        self.pieces.push(OutputPiece::PartOfLine {
            line: line.number,
            from: (line.offset + skip) as u64 + 1,
            to: (line.offset + of) as u64,
            of: line.characters as u64,
            text: shown,
        });
        Ok(())
    }

    /// A line shown in part is shown as text; images in it are named.
    fn images_left_out(&mut self, line: &Line) -> io::Result<()> {
        if self.graphics_noted == Some(line.number) {
            return Ok(());
        }
        let (images, graphics) = (line.images, line.graphics);
        if images > 0 || graphics > 0 {
            self.graphics_noted = Some(line.number);
            self.pieces.push(OutputPiece::Unreadable { line: line.number, reason: format!("{images} images and {graphics} graphics sequences that cannot be shown as images in a line too long to show whole; the log holds them") });
        }
        Ok(())
    }

    fn omitted(&mut self, first_line: u64, last_line: u64) {
        self.pieces.push(OutputPiece::Omitted { first_line, last_line });
    }

    fn text(&mut self, line: u64, text: &str) {
        self.pieces.text(line, text);
    }

    fn count(&mut self, line: &Line) {
        if line.decoding.escaped {
            self.escaped.add(line.number);
        }
        for name in &line.decoding.legacy {
            self.legacy.entry(name.clone()).or_default().add(line.number);
        }
    }

    fn finish(self, log: &Path, bytes: u64, lines: u64) -> OutputWindow {
        OutputWindow {
            log: log.to_string_lossy().into_owned(),
            bytes,
            lines,
            pieces: self.pieces.finish(),
            legacy: self
                .legacy
                .into_iter()
                .map(|(encoding, lines)| LegacyLines { encoding, lines: lines.count })
                .collect(),
            escaped_lines: self.escaped.count,
            failed: Vec::new(),
        }
    }
}

/// Fragments arrive in raw-line order, so deduplication needs only the last
/// counted number rather than a set whose size follows the log.
#[derive(Default)]
struct LineCount {
    last: Option<u64>,
    count: u64,
}
impl LineCount {
    fn add(&mut self, line: u64) {
        if self.last != Some(line) {
            self.last = Some(line);
            self.count += 1;
        }
    }
}

fn encode_image(bytes: &crate::spool::Bytes) -> io::Result<String> {
    let length =
        base64::encoded_len(bytes.len(), true).ok_or_else(|| io::Error::other("image encoding length overflow"))?;
    let mut encoded = base64::write::EncoderWriter::new(Vec::with_capacity(length), &STANDARD);
    io::copy(&mut bytes.reader(), &mut encoded)?;
    String::from_utf8(encoded.finish()?).map_err(io::Error::other)
}

fn text_cost(line: &Line) -> io::Result<usize> {
    let mut total = 0usize;
    for part in line.parts.iter() {
        total = total.saturating_add(match part? {
            Part::Text(text) => text.chars().count(),
            Part::Spaces(count) => count,
            _ => 0,
        });
    }
    Ok(total)
}

/// Copies only the requested characters, without expanding sparse gaps.
pub(super) fn line_slice(line: &Line, mut skip: usize, mut take: usize) -> io::Result<String> {
    let mut out = String::new();
    for part in line.parts.iter() {
        let part = part?;
        let size = match &part {
            Part::Text(text) => text.chars().count(),
            Part::Spaces(count) => *count,
            _ => continue,
        };
        if skip >= size {
            skip -= size;
            continue;
        }
        let count = take.min(size - skip);
        match &part {
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
    Ok(out)
}
