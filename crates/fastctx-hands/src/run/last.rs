//! The end of a job's output, told with the word that the job ended, so the
//! resident knows how it went without reading the log.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;

use fastctx_types::residents::LastLines;

use super::terminal::{Lines, Part};

/// Lines told at most.
pub const LINES: usize = 20;
/// Characters told at most.
pub const CHARACTERS: usize = 2_000;

/// The last lines of the output at `log` as a terminal shows them, read
/// from the file's tail; `None` when it is empty.
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

fn tail(log: &Path, line_limit: usize, character_limit: usize) -> io::Result<Option<LastLines>> {
    let mut file = File::open(log)?;
    let size = file.metadata()?.len();
    if size == 0 {
        return Ok(None);
    }
    // A character takes at most four bytes; colour sequences a terminal
    // drops take more, and when they crowd the tail fewer lines are told.
    let take = (character_limit as u64) * 4 + 4096;
    let start = size.saturating_sub(take);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut tail = bytes.as_slice();
    if start > 0 {
        // The first line read is likely the end of a longer one.
        tail = tail.iter().position(|&byte| byte == b'\n').map_or(&[][..], |at| &tail[at + 1..]);
    }
    let mut lines = Lines::new(tail);
    let mut kept: VecDeque<String> = VecDeque::new();
    let mut cost = 0;
    let mut read = 0;
    let mut previous = None;
    while let Some(line) = lines.next_line()? {
        let text: String = line
            .parts
            .iter()
            .map(|part| match part {
                Part::Text(text) => text.clone(),
                // Only a suffix can survive the preview budget. One extra
                // column makes the existing shortening notice mandatory.
                Part::Spaces(count) => " ".repeat((*count).min(character_limit + 1)),
                Part::Image(image) => format!("[an image, {}x{}]", image.width, image.height),
                Part::Unreadable(why) => format!("[{why}]"),
            })
            .collect();
        cost += text.chars().count();
        if previous == Some(line.number) {
            kept.back_mut().expect("previous fragment").push_str(&text);
        } else {
            read += 1;
            cost += 1;
            kept.push_back(text);
        }
        previous = Some(line.number);
        while kept.len() > line_limit || (cost > character_limit && kept.len() > 1) {
            cost -= kept.pop_front().expect("more than one").chars().count() + 1;
        }
    }
    let whole = start == 0 && kept.len() == read;
    let mut text = kept.into_iter().collect::<Vec<_>>().join("\n");
    if text.chars().count() > character_limit {
        // One last line longer than all that is told: its end.
        let skip = text.chars().count() - (character_limit - 1);
        text = format!("…{}", text.chars().skip(skip).collect::<String>());
        return Ok(Some(LastLines { text, whole: false }));
    }
    Ok((!text.is_empty()).then_some(LastLines { text, whole }))
}
