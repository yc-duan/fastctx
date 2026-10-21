//! Assemble only requested rows; the holding machine sends bounded pieces.

use crate::line_rows::{Rows, Sink};
use crate::{CallError, Error, World};
use fastctx_types::access::Written;
use fastctx_types::ipc::{ClientFrame, NodeFrame};
use fastctx_types::lines::Reply as LinesReply;
use fastctx_types::{Call, Reply};

impl World {
    pub(crate) async fn read_lines(
        &self,
        machine: Written,
        source: String,
        start: u64,
        count: u64,
    ) -> Result<LinesReply, Error> {
        let mut rows = Rows::new(start, count);
        let reply = self.visit_lines(machine, source, start, count, &mut rows).await?;
        rows.finish(reply)
    }

    pub(crate) async fn visit_lines(
        &self,
        machine: Written,
        source: String,
        start: u64,
        count: u64,
        receive: &mut (impl Sink + Send),
    ) -> Result<LinesReply, Error> {
        self.visit_line_parts(machine, source, start, count, receive, false).await
    }

    pub(crate) async fn visit_line_notes(
        &self,
        machine: Written,
        source: String,
        count: u64,
        receive: &mut (impl Sink + Send),
    ) -> Result<LinesReply, Error> {
        self.visit_line_parts(machine, source, 0, count, receive, true).await
    }

    async fn visit_line_parts(
        &self,
        machine: Written,
        source: String,
        start: u64,
        count: u64,
        receive: &mut (impl Sink + Send),
        only_notes: bool,
    ) -> Result<LinesReply, Error> {
        let reply = if let Some(device) = self.device() {
            let mut reading = device.read_lines(machine, source, start, count, only_notes)?;
            loop {
                match reading.next().await? {
                    fastctx_net::lines_read::Received::Text(part) => receive.put(part).await?,
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
                    call: if only_notes {
                        Call::ReadLineNotes {
                            machine: Some(machine),
                            request: fastctx_types::lines::ReadRequest { source, start, count },
                        }
                    } else {
                        Call::ReadLines {
                            machine: Some(machine),
                            request: fastctx_types::lines::ReadRequest { source, start, count },
                        }
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
                    NodeFrame::LinesRead { id: answered, part } if answered == id => receive.put(part).await?,
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
        Ok(reply)
    }
}

fn broken(message: &str) -> Error {
    CallError::Broken(std::io::Error::new(std::io::ErrorKind::InvalidData, message)).into()
}
