//! Following Claude Code's session file as it grows, each whole line once.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

#[derive(Default)]
pub struct Tail {
    offset: u64,
    /// The last line read and where it starts: a file that no longer holds
    /// it there was rewritten, and is read again from its start.
    anchor: Option<(u64, Vec<u8>)>,
}

/// What a read of the file finds, in order.
pub enum Found<'a> {
    /// The file was rewritten since the last read; what follows is read
    /// again from its start.
    Restarted,
    Row(&'a Value),
}

impl Tail {
    /// Hands `each` every row written since the last call, in order. A line
    /// still being written waits for the next call.
    pub fn read(&mut self, path: &Path, mut each: impl FnMut(Found<'_>) -> io::Result<()>) -> io::Result<()> {
        let mut file = File::open(path)?;
        let mut rewritten = file.metadata()?.len() < self.offset;
        if !rewritten && let Some((offset, line)) = &self.anchor {
            file.seek(SeekFrom::Start(*offset))?;
            let mut current = Vec::new();
            BufReader::new(&mut file).read_until(b'\n', &mut current)?;
            rewritten = &current != line;
        }
        if rewritten {
            *self = Self::default();
            each(Found::Restarted)?;
        }
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.offset))?;
        let mut line = Vec::new();
        loop {
            line.clear();
            let length = reader.read_until(b'\n', &mut line)?;
            if length == 0 || line.last() != Some(&b'\n') {
                return Ok(());
            }
            let row: Value = serde_json::from_slice(&line).map_err(io::Error::other)?;
            each(Found::Row(&row))?;
            self.anchor = Some((self.offset, line.clone()));
            self.offset += length as u64;
        }
    }
}
