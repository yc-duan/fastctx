//! Assemble logical rows from ordered, projected text parts.

use crate::{CallError, Error};
use fastctx_types::lines::{LineNote, Part, Reply as LinesReply, Result as Answer};

pub(crate) struct Rows {
    start: u64,
    count: u64,
    lines: Vec<String>,
    pending: String,
    notes: Vec<LineNote>,
    units: u64,
    unfinished: bool,
}

pub(crate) trait Sink {
    fn put(&mut self, part: Part) -> impl std::future::Future<Output = Result<(), Error>> + Send;
}

impl Sink for Rows {
    async fn put(&mut self, part: Part) -> Result<(), Error> {
        self.push(part)
    }
}
impl Rows {
    pub(crate) fn new(start: u64, count: u64) -> Self {
        Self { start, count, lines: Vec::new(), pending: String::new(), notes: Vec::new(), units: 0, unfinished: false }
    }

    pub(crate) fn push(&mut self, part: Part) -> Result<(), Error> {
        if part.utf16_length.is_some() {
            return Err(broken("line annotations arrived where text was requested"));
        }
        if self.lines.len() as u64 >= self.count || part.line != self.start + self.lines.len() as u64 {
            return Err(broken("line piece is outside the requested range or out of order"));
        }
        let end = self.units + part.text.encode_utf16().count() as u64;
        if let Some(note) = part.note.filter(|_| end > self.units) {
            if let Some(last) = self.notes.last_mut()
                && last.line == part.line
                && last.to == self.units
                && last.note == note
            {
                last.to = end;
            } else {
                self.notes.push(LineNote { line: part.line, from: self.units, to: end, note });
            }
        }
        self.units = end;
        self.pending.push_str(&part.text);
        self.unfinished = !part.ended;
        if part.ended {
            self.lines.push(std::mem::take(&mut self.pending));
            self.units = 0;
        }
        Ok(())
    }

    pub(crate) fn into_lines(self) -> Result<(Vec<String>, Vec<LineNote>), Error> {
        if self.unfinished {
            return Err(broken("the kept lines end in an unfinished part"));
        }
        Ok((self.lines, self.notes))
    }
    pub(crate) fn finish(self, mut reply: LinesReply) -> Result<LinesReply, Error> {
        if reply.result != (Answer::ReadComplete { start: self.start, count: self.lines.len() as u64 })
            || self.unfinished
        {
            return Err(broken("line completion disagrees with the received rows"));
        }
        reply.result = Answer::Lines { start: self.start, lines: self.lines, notes: self.notes };
        Ok(reply)
    }
}
fn broken(message: &str) -> Error {
    CallError::Broken(std::io::Error::new(std::io::ErrorKind::InvalidData, message)).into()
}
