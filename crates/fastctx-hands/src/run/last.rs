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
/// Characters told at most.
pub const CHARACTERS: usize = 2_000;

/// The last lines of the output at `log` as a terminal shows them, read
/// from a fixed-size snapshot; `None` when its rendered text is empty.
pub fn last_lines(log: &Path) -> io::Result<Option<LastLines>> {
    tail(log, LINES, CHARACTERS)
}

/// Separate streams have no shared byte order. Give each half of the
/// existing preview budget and label them rather than inventing an order.
pub fn last_streams(output: &Path, stderr: Option<&Path>) -> io::Result<Option<LastLines>> {
    let Some(stderr) = stderr else { return last_lines(output) };
    let mut parts = Vec::new();
    let mut whole = true;
    for (label, path) in [("stdout:\n", output), ("stderr:\n", stderr)] {
        if let Some(last) = tail(path, LINES / 2 - 1, (CHARACTERS - 1) / 2 - label.len())? {
            whole &= last.whole;
            parts.push(format!("{label}{}", last.text));
        }
    }
    Ok((!parts.is_empty()).then(|| LastLines { text: parts.join("\n"), whole }))
}

struct Preview {
    tail: VecDeque<char>,
    characters: usize,
}
impl Preview {
    fn push(&mut self, text: &str, limit: usize) {
        let characters = text.chars().count();
        self.characters = self.characters.saturating_add(characters);
        if characters >= limit {
            self.tail.clear();
            self.tail.extend(text.chars().rev().take(limit));
            self.tail.make_contiguous().reverse();
            return;
        }
        for c in text.chars() {
            self.tail.push_back(c);
            if self.tail.len() > limit {
                self.tail.pop_front();
            }
        }
    }
    fn spaces(&mut self, count: usize, limit: usize) {
        self.characters = self.characters.saturating_add(count);
        if count >= limit {
            self.tail.clear();
        }
        for _ in 0..count.min(limit) {
            self.tail.push_back(' ');
            if self.tail.len() > limit {
                self.tail.pop_front();
            }
        }
    }
}

fn tail(log: &Path, line_limit: usize, character_limit: usize) -> io::Result<Option<LastLines>> {
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
            kept.push_back(Preview { tail: VecDeque::new(), characters: 0 });
        }
        previous = Some(line.number);
        let preview = kept.back_mut().expect("current line");
        let before = preview.characters;
        for part in line.parts.iter() {
            match part? {
                Part::Text(text) => preview.push(&text, character_limit),
                Part::Spaces(count) => preview.spaces(count, character_limit),
                Part::Image(image) => {
                    preview.push(&format!("[an image, {}x{}]", image.width, image.height), character_limit)
                }
                Part::Unreadable(why) => preview.push(&format!("[{why}]"), character_limit),
            }
        }
        cost = cost.saturating_add(preview.characters - before);
        while kept.len() > line_limit || (cost > character_limit && kept.len() > 1) {
            cost = cost.saturating_sub(kept.pop_front().expect("more than one").characters.saturating_add(1));
        }
    }
    let mut whole = kept.len() as u64 == read;
    let mut text = if kept.len() == 1 && kept[0].characters > character_limit {
        whole = false;
        let last = kept.pop_front().expect("one");
        format!("…{}", last.tail.into_iter().skip(1).collect::<String>())
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
        let available = character_limit - note.chars().count() - 1;
        if text.chars().count() > available {
            text = format!(
                "…{}",
                text.chars().rev().take(available - 1).collect::<Vec<_>>().into_iter().rev().collect::<String>()
            );
            whole = false;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&note);
    }
    Ok((!text.is_empty()).then_some(LastLines { text, whole }))
}
