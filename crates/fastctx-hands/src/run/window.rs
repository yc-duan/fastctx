//! The part of a job's output one answer shows.
//!
//! Files the resident asked to read with `fastctx view` are shown whole up
//! to the answer's budget (`world-view.md`, 一次给多少). Any other output is
//! shown in the window a host's own shell gives: when it does not all fit,
//! the answer shows the start and the end and names the lines in between,
//! which the resident reads back from the log with `fastctx view --lines`.

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_types::run::{LegacyLines, OutputPiece, OutputWindow};

use super::terminal::{Line, Lines, Part, VIEW_MARK};

/// Characters the complete answer may hold, including head notes and
/// delivered words, with each image counted as 5,200 (`world-view.md`).
pub const BUDGET: usize = 150_000;
pub const IMAGE_COST: usize = 5_200;

/// Raw image bytes one answer may carry. Claude Code runs on Node, where a
/// string holds at most about 5.4 × 10⁸ characters; the whole answer is one
/// JSON line and base64 makes images a third larger, so 400 MiB of images
/// is about the most an answer can carry and still be read (`world-view.md`).
pub const IMAGE_BYTES: usize = 400 * 1024 * 1024;

/// Characters a program's output may take in one answer: what Claude Code's
/// own shell tool shows by default, and about the 10,000 tokens Codex gives a
/// command's output, so either model reads it as it reads its host's shell.
pub const OUTPUT_WINDOW: usize = 30_000;

/// What `run`'s own notes can take beside a view's output: the first line,
/// a machine name of at most 32 characters with the facts of how the script
/// ended, and the note naming the log when processes it started go on.
pub const RUN_NOTES: usize = 1_024;

/// The part of the output at `log` one answer shows: what `fastctx view`
/// wrote up to the whole budget, any other output in `OUTPUT_WINDOW`.
pub fn window(log: &Path) -> io::Result<OutputWindow> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(log)?;
    let bytes = file.metadata()?.len();
    // Which regions `fastctx view` wrote and closed, read in a pass of its
    // own; output that never ran it, nearly all, skips that pass.
    let closed_views = if holds_view_mark(&mut file)? {
        file.seek(SeekFrom::Start(0))?;
        let mut scan = Lines::new((&mut file).take(bytes));
        while scan.next_line()?.is_some() {}
        scan.closed_views().clone()
    } else {
        Default::default()
    };
    file.seek(SeekFrom::Start(0))?;
    let mut regular = Selection::new(OUTPUT_WINDOW);
    let mut lines = Lines::new((&mut file).take(bytes));
    let mut id = 0;
    while let Some(line) = lines.next_line()? {
        if !closed_views.contains(&line.view) {
            regular.push(id, cost(&line));
        }
        id += 1;
    }
    let (chosen, used) = regular.finish();
    let mut view_left = (BUDGET - RUN_NOTES).saturating_sub(used);
    file.seek(SeekFrom::Start(0))?;
    let mut lines = Lines::new(file.take(bytes));
    let mut assembly = Assembly::default();
    let mut omitted: Option<(u64, u64)> = None;
    id = 0;
    while let Some(line) = lines.next_line()? {
        let portions = if closed_views.contains(&line.view) {
            let size = cost(&line);
            let portions = if size <= view_left {
                vec![Portion::Whole]
            } else if view_left > 0 {
                vec![Portion::Start(view_left)]
            } else {
                Vec::new()
            };
            view_left = view_left.saturating_sub(size);
            portions
        } else {
            chosen.get(&id).cloned().unwrap_or_default()
        };
        if portions.is_empty() && cost(&line) > 0 {
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
    Ok(assembly.finish(log, bytes, lines.raw_lines()))
}

/// Whether the log holds the mark `fastctx view` writes first, read in
/// chunks that overlap by the mark's length less one.
fn holds_view_mark(file: &mut File) -> io::Result<bool> {
    use std::io::Read;
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
    Start(usize),
    End(usize),
}

/// Retains only positions and costs while scanning. Large lines and image
/// payloads need not remain in memory just to select the start and end.
struct Selection {
    budget: usize,
    head: Vec<(u64, usize)>,
    head_cost: usize,
    tail: VecDeque<(u64, usize)>,
    tail_cost: usize,
    first_cut: Option<u64>,
    overflowed: bool,
}

impl Selection {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            head: Vec::new(),
            head_cost: 0,
            tail: VecDeque::new(),
            tail_cost: 0,
            first_cut: None,
            overflowed: false,
        }
    }

    fn push(&mut self, id: u64, size: usize) {
        if size == 0 {
            return;
        }
        if !self.overflowed {
            if size <= self.budget - self.head_cost {
                self.head.push((id, size));
                self.head_cost += size;
                return;
            }
            self.overflowed = true;
            let mut fitting = 0;
            self.head_cost = 0;
            for &(_, cost) in &self.head {
                if cost > self.budget / 4 - self.head_cost {
                    break;
                }
                self.head_cost += cost;
                fitting += 1;
            }
            for entry in self.head.split_off(fitting) {
                self.tail_cost += entry.1;
                self.tail.push_back(entry);
            }
            if self.head.is_empty() {
                self.first_cut = Some(self.tail.front().map_or(id, |entry| entry.0));
                self.head_cost = self.budget / 4;
            }
        }
        // Evict before adding so even a sparse line costing usize::MAX
        // cannot overflow the accumulated cost.
        while !self.tail.is_empty() && size > (self.budget - self.head_cost).saturating_sub(self.tail_cost) {
            self.tail_cost -= self.tail.pop_front().expect("nonempty").1;
        }
        self.tail_cost = self.tail_cost.saturating_add(size);
        self.tail.push_back((id, size));
    }

    fn finish(self) -> (std::collections::BTreeMap<u64, Vec<Portion>>, usize) {
        let mut chosen = std::collections::BTreeMap::<u64, Vec<Portion>>::new();
        let mut used = 0;
        for (id, size) in self.head {
            chosen.entry(id).or_default().push(Portion::Whole);
            used += size;
        }
        let tail_budget = self.budget - self.head_cost;
        let last_cut = self.tail.len() == 1 && self.tail_cost > tail_budget;
        if let Some(id) = self.first_cut.filter(|id| self.tail.front().map(|entry| entry.0) != Some(*id) || last_cut) {
            chosen.entry(id).or_default().push(Portion::Start(self.budget / 4));
            used += self.budget / 4;
        }
        for (id, size) in self.tail {
            chosen.entry(id).or_default().push(if last_cut { Portion::End(tail_budget) } else { Portion::Whole });
            used += if last_cut { tail_budget } else { size };
        }
        (chosen, used)
    }
}

/// What a line costs against the budget: its characters and its newline,
/// with images at their fixed cost.
fn cost(line: &Line) -> usize {
    usize::from(line.newline).saturating_add(
        line.parts
            .iter()
            .map(|part| match part {
                Part::Text(text) => text.chars().count(),
                Part::Spaces(count) => *count,
                Part::Image(_) => IMAGE_COST,
                Part::Unreadable(reason) => reason.chars().count(),
            })
            .fold(0usize, usize::saturating_add),
    )
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
