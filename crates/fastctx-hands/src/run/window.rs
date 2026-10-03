//! The part of a job's output one answer shows.
//!
//! Output is shown faithfully from the top until the budget is reached
//! (`world-view.md`, 一次给多少). When it does not all fit, the answer shows
//! the start and the end and names the lines in between, which the resident
//! reads back from the log with `fastctx view --lines`. The budget exists
//! only so that one answer cannot break the resident's session; it is the
//! same for every model.

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_types::run::{LegacyLines, OutputPiece, OutputWindow};

use super::terminal::{Decoding, Line, Lines, Part};

/// Characters one answer may hold: 150,000, with each image counted as
/// 5,200 (`world-view.md`).
pub const BUDGET: usize = 150_000;
pub const IMAGE_COST: usize = 5_200;

/// Raw image bytes one answer may carry. Claude Code runs on Node, where a
/// string holds at most about 5.4 × 10⁸ characters; the whole answer is one
/// JSON line and base64 makes images a third larger, so 400 MiB of images
/// is about the most an answer can carry and still be read (`world-view.md`).
pub const IMAGE_BYTES: usize = 400 * 1024 * 1024;

/// The window onto the log at `log` for an answer of at most `budget`
/// characters. The start gets a quarter of the budget, the end the rest:
/// what went wrong usually shows last.
pub fn window(log: &Path, budget: usize) -> io::Result<OutputWindow> {
    let file = File::open(log)?;
    let bytes = file.metadata()?.len();
    let head_budget = budget / 4;
    let mut lines = Lines::new(file);

    // While everything fits, `head` holds every line. Once it does not, it
    // keeps the lines that fit the head's share and the rest go to `tail`,
    // which from then on keeps only the latest lines that fit.
    let mut head: Vec<Line> = Vec::new();
    let mut head_cost = 0;
    let mut first_cut: Option<Line> = None;
    let mut tail: VecDeque<Line> = VecDeque::new();
    let mut tail_cost = 0;
    let mut overflowed = false;
    let mut total = 0;
    while let Some(line) = lines.next_line()? {
        total = line.number;
        let line_cost = cost(&line);
        if !overflowed {
            if head_cost + line_cost <= budget {
                head_cost += line_cost;
                head.push(line);
                continue;
            }
            overflowed = true;
            let mut fitting = 0;
            head_cost = 0;
            for kept in &head {
                if head_cost + cost(kept) > head_budget {
                    break;
                }
                head_cost += cost(kept);
                fitting += 1;
            }
            for rest in head.split_off(fitting) {
                tail_cost += cost(&rest);
                tail.push_back(rest);
            }
            if head.is_empty() {
                // Not even the first line fits the head's share: its start
                // is shown instead.
                first_cut = Some(tail.front().cloned().unwrap_or_else(|| line.clone()));
                head_cost = head_budget;
            }
        }
        tail_cost += line_cost;
        tail.push_back(line);
        let tail_budget = budget - head_cost;
        while tail_cost > tail_budget && tail.len() > 1 {
            tail_cost -= cost(&tail.pop_front().expect("more than one"));
        }
    }

    let mut assembly = Assembly::default();
    if !overflowed {
        for line in &head {
            assembly.whole(line);
        }
    } else {
        let tail_budget = budget - head_cost;
        // A last line too long for the tail's share shows its end.
        let last_cut = (tail.len() == 1 && tail_cost > tail_budget).then(|| tail.pop_back().expect("one line"));
        let tail_first = tail.front().or(last_cut.as_ref()).map(|line| line.number);
        // The first line shown whole in the tail needs no cut start too.
        let first_cut = first_cut.filter(|line| Some(line.number) != tail_first || last_cut.is_some());
        for line in &head {
            assembly.whole(line);
        }
        if let Some(line) = &first_cut {
            assembly.start_of(line, head_budget);
        }
        let shown_until = head.last().map(|line| line.number).or(first_cut.as_ref().map(|line| line.number)).unwrap_or(0);
        let shown_from = tail_first.unwrap_or(total + 1);
        if shown_from > shown_until + 1 {
            assembly.omitted(shown_until + 1, shown_from - 1);
        }
        for line in &tail {
            assembly.whole(line);
        }
        if let Some(line) = &last_cut {
            assembly.end_of(line, tail_budget);
        }
    }
    Ok(assembly.finish(log, bytes, total))
}

/// What a line costs against the budget: its characters and its newline,
/// with images at their fixed cost.
fn cost(line: &Line) -> usize {
    1 + line
        .parts
        .iter()
        .map(|part| match part {
            Part::Text(text) => text.chars().count(),
            Part::Image(_) => IMAGE_COST,
            Part::Unreadable(reason) => reason.chars().count(),
        })
        .sum::<usize>()
}

#[derive(Default)]
struct Assembly {
    pieces: Vec<OutputPiece>,
    image_bytes: usize,
    legacy: Option<LegacyLines>,
    escaped_lines: u64,
}

impl Assembly {
    fn whole(&mut self, line: &Line) {
        self.count(line);
        for part in &line.parts {
            match part {
                Part::Text(text) => self.text(line.number, text),
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
        self.text(line.number, "\n");
    }

    /// The first `budget` characters of a line too long to show whole.
    fn start_of(&mut self, line: &Line, budget: usize) {
        self.count(line);
        self.images_left_out(line);
        let text = line_text(line);
        let of = text.chars().count();
        let shown: String = text.chars().take(budget).collect();
        let to = shown.chars().count();
        self.pieces.push(OutputPiece::PartOfLine { line: line.number, from: 1, to: to as u64, of: of as u64, text: shown });
    }

    /// The last `budget` characters of a line too long to show whole.
    fn end_of(&mut self, line: &Line, budget: usize) {
        self.count(line);
        self.images_left_out(line);
        let text = line_text(line);
        let of = text.chars().count();
        let skip = of.saturating_sub(budget);
        let shown: String = text.chars().skip(skip).collect();
        self.pieces.push(OutputPiece::PartOfLine {
            line: line.number,
            from: skip as u64 + 1,
            to: of as u64,
            of: of as u64,
            text: shown,
        });
    }

    /// A line shown in part is shown as text; images in it are named.
    fn images_left_out(&mut self, line: &Line) {
        let images = line.parts.iter().filter(|part| !matches!(part, Part::Text(_))).count();
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
        match &line.decoding {
            Decoding::Utf8 => {}
            Decoding::Escaped => self.escaped_lines += 1,
            Decoding::Legacy(name) => {
                self.legacy.get_or_insert_with(|| LegacyLines { encoding: name.clone(), lines: 0 }).lines += 1;
            }
        }
    }

    fn finish(mut self, log: &Path, bytes: u64, lines: u64) -> OutputWindow {
        // The output's own last newline ends it; it is not shown as an
        // empty line.
        if let Some(OutputPiece::Text { text, .. }) = self.pieces.last_mut()
            && text.ends_with('\n')
        {
            text.pop();
        }
        self.pieces.retain(|piece| !matches!(piece, OutputPiece::Text { text, .. } if text.is_empty()));
        OutputWindow {
            log: log.to_string_lossy().into_owned(),
            bytes,
            lines,
            pieces: self.pieces,
            legacy: self.legacy,
            escaped_lines: self.escaped_lines,
        }
    }
}

/// A line's text alone, for showing part of it.
fn line_text(line: &Line) -> String {
    line.parts
        .iter()
        .filter_map(|part| match part {
            Part::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}
