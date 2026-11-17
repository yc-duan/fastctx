//! Following Claude Code's session file as it grows, each whole line once.

use std::fs::File;
use std::io::{self, BufReader, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

#[derive(Default)]
pub struct Tail {
    offset: u64,
    /// The last line read and where it starts: a file that no longer holds
    /// it there was rewritten, and is read again from its start.
    anchor: Option<(u64, Vec<u8>)>,
    /// Passes over a line larger than World reads, where it otherwise stops
    /// at it (`passing_large`).
    passing: bool,
}

/// What a read of the file finds, in order.
pub enum Found<'a> {
    /// The file was rewritten since the last read; what follows is read
    /// again from its start.
    Restarted,
    Row(&'a Value),
    /// A line too large to read, starting `at`, gone over unread
    /// (`Tail::passing_large`): it may have been a row looked for.
    Unread { at: u64 },
}

impl Tail {
    /// A tail that passes over a line larger than World reads (design
    /// §28.49) without reading it, for a reader that looks for small rows
    /// among others — a Codex thread's compactions — and would rather miss
    /// a row of that size than every row after it.
    pub fn passing_large() -> Self {
        Self { passing: true, ..Self::default() }
    }

    /// Hands `each` every row written since the last call, in order. A line
    /// still being written waits for the next call.
    pub fn read(&mut self, path: &Path, mut each: impl FnMut(Found<'_>) -> io::Result<()>) -> io::Result<()> {
        let mut file = File::open(path)?;
        let mut rewritten = file.metadata()?.len() < self.offset;
        if !rewritten && let Some((offset, line)) = &self.anchor {
            file.seek(SeekFrom::Start(*offset))?;
            let mut current = Vec::new();
            // A line too large to read where the last one read stood is not it.
            rewritten = match crate::bounded::read_bounded(&mut BufReader::new(&mut file), &mut current)? {
                crate::bounded::Read::Line(_) => &current != line,
                crate::bounded::Read::Refused { .. } => true,
            };
        }
        if rewritten {
            *self = Self { passing: self.passing, ..Self::default() };
            each(Found::Restarted)?;
        }
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.offset))?;
        let mut line = Vec::new();
        loop {
            line.clear();
            // A line past the bound stops the reading, as one that is not
            // JSON does (design §28.49) — or, passing, is gone over unread,
            // once it is whole, the last line read still the anchor.
            let length = match crate::bounded::read_bounded(&mut reader, &mut line)? {
                crate::bounded::Read::Line(length) => length,
                crate::bounded::Read::Refused { ended } if self.passing => {
                    if !ended && !crate::bounded::skip_line(&mut reader)? {
                        return Ok(());
                    }
                    each(Found::Unread { at: self.offset })?;
                    self.offset = reader.stream_position()?;
                    continue;
                }
                crate::bounded::Read::Refused { .. } => {
                    return Err(crate::bounded::refused(0, &format!("is longer than {} bytes", crate::bounded::LINE_MOST)));
                }
            };
            if length == 0 || line.last() != Some(&b'\n') {
                return Ok(());
            }
            if crate::bounded::too_many_values(&line) {
                if self.passing {
                    each(Found::Unread { at: self.offset })?;
                    self.offset += length as u64;
                    continue;
                }
                return Err(crate::bounded::refused(0, &format!("holds more than {} values", crate::bounded::VALUES_MOST)));
            }
            let row: Value = serde_json::from_slice(&line).map_err(io::Error::other)?;
            each(Found::Row(&row))?;
            self.anchor = Some((self.offset, line.clone()));
            self.offset += length as u64;
        }
    }
}
