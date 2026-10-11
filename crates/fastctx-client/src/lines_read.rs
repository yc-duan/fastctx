//! Assemble only requested rows; the holding machine sends bounded pieces.

use crate::{CallError, Error, World};
use fastctx_types::ipc::{ClientFrame, NodeFrame};
use fastctx_types::lines::{LineNote, Part, Reply as LinesReply, Result as Answer};
use fastctx_types::{Call, MachineName, Reply};

impl World {
    pub(crate) async fn read_lines(
        &self,
        machine: MachineName,
        source: String,
        start: u64,
        count: u64,
    ) -> Result<LinesReply, Error> {
        let mut rows = Rows { start, count, lines: Vec::new(), pending: String::new(), notes: Vec::new(), units: 0 };
        let reply = if let Some(device) = self.device() {
            let mut reading = device.read_lines(machine, source, start, count)?;
            loop {
                match reading.next().await? {
                    fastctx_net::lines_read::Received::Text(part) => rows.push(part)?,
                    fastctx_net::lines_read::Received::Finished(reply) => break reply,
                }
            }
        } else {
            let mut client = self.connect().await?;
            client.next += 1;
            let id = client.next;
            client
                .connection
                .send(&crate::to_json(&ClientFrame::Call {
                    id,
                    call: Call::ReadLines {
                        machine: Some(machine),
                        request: fastctx_types::lines::ReadRequest { source, start, count },
                    },
                }))
                .await
                .map_err(CallError::Broken)?;
            loop {
                let frame = client
                    .connection
                    .recv()
                    .await
                    .map_err(CallError::Broken)?
                    .ok_or_else(|| broken("line read ended before completion"))?;
                let frame: NodeFrame = serde_json::from_slice(&frame).map_err(|error| broken(&error.to_string()))?;
                match frame {
                    NodeFrame::LinesRead { id: answered, part } if answered == id => rows.push(part)?,
                    NodeFrame::Reply { id: answered, reply } if answered == id => {
                        let Reply::Lines(reply) = reply.map_err(CallError::Failed)? else {
                            return Err(broken("expected line-read completion"));
                        };
                        break reply;
                    }
                    _ => return Err(broken("unexpected line-read frame")),
                }
            }
        };
        rows.finish(reply)
    }
}

struct Rows {
    start: u64,
    count: u64,
    lines: Vec<String>,
    pending: String,
    notes: Vec<LineNote>,
    units: u64,
}
impl Rows {
    fn push(&mut self, part: Part) -> Result<(), Error> {
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
        if part.ended {
            self.lines.push(std::mem::take(&mut self.pending));
            self.units = 0;
        }
        Ok(())
    }
    fn finish(self, mut reply: LinesReply) -> Result<LinesReply, Error> {
        if reply.result != (Answer::ReadComplete { start: self.start, count: self.lines.len() as u64 })
            || !self.pending.is_empty()
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
