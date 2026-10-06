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
    let mut file = File::open(log)?;
    let size = file.metadata()?.len();
    if size == 0 {
        return Ok(None);
    }
    // A character takes at most four bytes; colour sequences a terminal
    // drops take more, and when they crowd the tail fewer lines are told.
    let take = (CHARACTERS as u64) * 4 + 4096;
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
    while let Some(line) = lines.next_line()? {
        read += 1;
        let text: String = line
            .parts
            .iter()
            .map(|part| match part {
                Part::Text(text) => text.clone(),
                Part::Image(image) => format!("[an image, {}x{}]", image.width, image.height),
                Part::Unreadable(why) => format!("[{why}]"),
            })
            .collect();
        cost += text.chars().count() + 1;
        kept.push_back(text);
        while kept.len() > LINES || (cost > CHARACTERS && kept.len() > 1) {
            cost -= kept.pop_front().expect("more than one").chars().count() + 1;
        }
    }
    let whole = start == 0 && kept.len() == read;
    let mut text = kept.into_iter().collect::<Vec<_>>().join("\n");
    if text.chars().count() > CHARACTERS {
        // One last line longer than all that is told: its end.
        let skip = text.chars().count() - CHARACTERS;
        text = format!("…{}", text.chars().skip(skip).collect::<String>());
        return Ok(Some(LastLines { text, whole: false }));
    }
    Ok((!text.is_empty()).then_some(LastLines { text, whole }))
}
