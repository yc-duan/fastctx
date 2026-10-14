//! The end of a job's output, told with the word that the job ended, so the
//! resident knows how it went without reading the log.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;

use fastctx_types::residents::LastLines;

use super::terminal::{Lines, Part};

/// Lines told at most.
pub const LINES: usize = 20;
/// Bytes of text told at most: the units an answer shows of the end of a
/// long output (`window::OUTPUT_TAIL`), so the two read alike.
pub const BYTES: usize = super::window::bytes(super::window::OUTPUT_TAIL);

/// The last lines of the output at `log` as a terminal shows them, read
/// from a fixed-size snapshot; `None` when its rendered text is empty.
pub fn last_lines(log: &Path) -> io::Result<Option<LastLines>> {
    tail(log, LINES, BYTES)
}

/// Separate streams have no shared byte order. Give each half of the
/// existing preview budget and label them rather than inventing an order.
pub fn last_streams(output: &Path, stderr: Option<&Path>) -> io::Result<Option<LastLines>> {
    let Some(stderr) = stderr else { return last_lines(output) };
    let mut parts = Vec::new();
    let mut whole = true;
    for (label, path) in [("stdout:\n", output), ("stderr:\n", stderr)] {
        if let Some(last) = tail(path, LINES / 2 - 1, (BYTES - 1) / 2 - label.len())? {
            whole &= last.whole;
            parts.push(format!("{label}{}", last.text));
        }
    }
    Ok((!parts.is_empty()).then(|| LastLines { text: parts.join("\n"), whole }))
}

/// The end of one line: its last characters within the limit, and how many
/// bytes the whole line holds.
struct Preview {
    tail: VecDeque<char>,
    tail_bytes: usize,
    bytes: usize,
}
impl Preview {
    fn keep(&mut self, c: char, limit: usize) {
        self.tail.push_back(c);
        self.tail_bytes += c.len_utf8();
        while self.tail_bytes > limit {
            let Some(first) = self.tail.pop_front() else { break };
            self.tail_bytes -= first.len_utf8();
        }
    }
    fn push(&mut self, text: &str, limit: usize) {
        self.bytes = self.bytes.saturating_add(text.len());
        if text.len() >= limit {
            // Only its own end can stay: read from the end, as far as fits.
            self.tail.clear();
            self.tail_bytes = 0;
            for c in text.chars().rev() {
                if self.tail_bytes + c.len_utf8() > limit {
                    break;
                }
                self.tail_bytes += c.len_utf8();
                self.tail.push_front(c);
            }
            return;
        }
        for c in text.chars() {
            self.keep(c, limit);
        }
    }
    fn spaces(&mut self, count: usize, limit: usize) {
        self.bytes = self.bytes.saturating_add(count);
        if count >= limit {
            self.tail.clear();
            self.tail_bytes = 0;
        }
        for _ in 0..count.min(limit) {
            self.keep(' ', limit);
        }
    }
}

/// The last characters of `chars` that fit in `room` bytes after a leading
/// ellipsis, which marks that the start was left out.
fn ellipsized(chars: impl DoubleEndedIterator<Item = char>, room: usize) -> String {
    let mut kept = VecDeque::new();
    let mut size = '…'.len_utf8();
    for c in chars.rev() {
        if size + c.len_utf8() > room {
            break;
        }
        size += c.len_utf8();
        kept.push_front(c);
    }
    format!("…{}", kept.into_iter().collect::<String>())
}

fn tail(log: &Path, line_limit: usize, byte_limit: usize) -> io::Result<Option<LastLines>> {
    let file = File::open(log)?;
    let size = file.metadata()?.len();
    let mut lines = Lines::new(file.take(size));
    let mut kept: VecDeque<Preview> = VecDeque::new();
    let mut cost = 0usize;
    let mut read = 0u64;
    let mut previous = None;
    while let Some(line) = lines.next_line()? {
        if previous != Some(line.number) {
            read += 1;
            cost = cost.saturating_add(1);
            kept.push_back(Preview { tail: VecDeque::new(), tail_bytes: 0, bytes: 0 });
        }
        previous = Some(line.number);
        let preview = kept.back_mut().expect("current line");
        let before = preview.bytes;
        for part in line.parts.iter() {
            match part? {
                Part::Text(text) => preview.push(&text, byte_limit),
                Part::Spaces(count) => preview.spaces(count, byte_limit),
                Part::Image(image) => {
                    preview.push(&format!("[an image, {}x{}]", image.width, image.height), byte_limit)
                }
                Part::Unreadable(why) => preview.push(&format!("[{why}]"), byte_limit),
            }
        }
        cost = cost.saturating_add(preview.bytes - before);
        while kept.len() > line_limit || (cost > byte_limit && kept.len() > 1) {
            cost = cost.saturating_sub(kept.pop_front().expect("more than one").bytes.saturating_add(1));
        }
    }
    let mut whole = kept.len() as u64 == read;
    let mut text = if kept.len() == 1 && kept[0].bytes > byte_limit {
        whole = false;
        let last = kept.pop_front().expect("one");
        ellipsized(last.tail.into_iter(), byte_limit)
    } else {
        kept.into_iter().map(|line| line.tail.into_iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    };
    if let Some(unfinished) = lines.unfinished() {
        let note = format!("[{}]", unfinished.description());
        let mut rows: Vec<_> = text.split('\n').collect();
        if rows.len() >= line_limit {
            rows.drain(..rows.len() - line_limit + 1);
            whole = false;
        }
        text = rows.join("\n");
        let available = byte_limit.saturating_sub(note.len() + 1);
        if text.len() > available {
            text = ellipsized(text.chars().collect::<Vec<_>>().into_iter(), available);
            whole = false;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&note);
    }
    Ok((!text.is_empty()).then_some(LastLines { text, whole }))
}
