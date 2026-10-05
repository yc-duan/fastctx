//! Original file bytes for interfaces. Each read owns its IPC connection;
//! dropping it cancels the read and propagates backpressure to the source.

use fastctx_types::file_read::{FileReadComplete, FileReadInfo, FileReadPart, ReadFileRequest};
use fastctx_types::ipc::{ClientFrame, NodeFrame};
use fastctx_types::{Call, MachineName, Reply};

use crate::{CallError, Client, Error, World};

impl World {
    /// Opens a file on `machine` (this node when absent) as the user.
    /// Consume `Reading::next` through its terminal result; bytes handed
    /// out before that are provisional if the read fails or changes.
    pub async fn read_file(&self, machine: Option<MachineName>, request: ReadFileRequest) -> Result<Reading, Error> {
        self.connect().await?.read_file(machine, request).await
    }
}

impl Client {
    /// Starts an exclusive byte stream on this connection.
    pub async fn read_file(mut self, machine: Option<MachineName>, request: ReadFileRequest) -> Result<Reading, Error> {
        self.next += 1;
        let id = self.next;
        let call = Call::ReadFile { machine, request };
        self.connection.send(&crate::to_json(&ClientFrame::Call { id, call })).await.map_err(CallError::Broken)?;
        match receive(&mut self).await? {
            NodeFrame::FileRead { id: answered, part: FileReadPart::Opened { info } } if answered == id => {
                if info.offset > info.file.size || info.length > info.file.size - info.offset {
                    return Err(broken("the node selected a range outside the observed file"));
                }
                Ok(Reading { client: self, id, info, received: 0, complete: None, ended: false })
            }
            NodeFrame::Reply { id: answered, reply } if answered == id => match reply.map_err(CallError::Failed)? {
                Reply::FileRead { result: Err(error) } => Err(error.into()),
                _ => Err(broken("the file read ended before its opening metadata")),
            },
            _ => Err(broken("the node answered something other than the file's opening metadata")),
        }
    }
}

/// A read's opening metadata and its original byte chunks. No whole-file
/// buffer is retained. `next` is not cancellation-safe: cancel by dropping
/// the entire `Reading`, rather than abandoning a partially received frame.
pub struct Reading {
    client: Client,
    id: u64,
    info: FileReadInfo,
    received: u64,
    complete: Option<FileReadComplete>,
    ended: bool,
}

impl Reading {
    pub fn info(&self) -> &FileReadInfo {
        &self.info
    }

    /// How many bytes have been handed to the caller, including after a
    /// failed read. Their offsets start at `info().offset`.
    pub fn received(&self) -> u64 {
        self.received
    }

    /// Present only after `next` returned `Ok(None)` with verified success.
    pub fn complete(&self) -> Option<&FileReadComplete> {
        self.complete.as_ref()
    }

    /// The next unchanged byte block. `Ok(None)` means a successful terminal
    /// result, never merely EOF. A failure terminates the stream; calling
    /// again after failure is an error, so failure cannot look like success.
    pub async fn next(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if self.ended {
            return if self.complete.is_some() { Ok(None) } else { Err(broken("this file read already failed")) };
        }
        let result = self.receive().await;
        if !matches!(result, Ok(Some(_))) {
            self.ended = true;
        }
        result
    }

    async fn receive(&mut self) -> Result<Option<Vec<u8>>, Error> {
        match receive(&mut self.client).await? {
            NodeFrame::FileRead { id, part: FileReadPart::Chunk { offset, bytes } } if id == self.id => {
                if offset != self.info.offset + self.received || bytes == 0 || bytes > self.info.length - self.received
                {
                    return Err(broken("the file read sent bytes outside the selected range or out of order"));
                }
                let data = self
                    .client
                    .connection
                    .recv()
                    .await
                    .map_err(CallError::Broken)?
                    .ok_or_else(|| broken("the file read ended in a binary block"))?;
                if data.len() as u64 != bytes {
                    return Err(broken("the binary block's length differs from its header"));
                }
                self.received += bytes;
                Ok(Some(data))
            }
            NodeFrame::Reply { id, reply } if id == self.id => {
                let Reply::FileRead { result } = reply.map_err(CallError::Failed)? else {
                    return Err(broken("the node answered something other than a file read"));
                };
                let complete = result.map_err(|error| {
                    if error.bytes != self.received {
                        broken("the file read's failure disagrees with the received byte count")
                    } else {
                        error.into()
                    }
                })?;
                if complete.bytes != self.received
                    || complete.bytes != self.info.length
                    || complete.file != self.info.file
                {
                    return Err(broken("the file read's completion disagrees with the received bytes or metadata"));
                }
                self.complete = Some(complete);
                Ok(None)
            }
            _ => Err(broken("the node sent an unexpected frame in a file read")),
        }
    }
}

async fn receive(client: &mut Client) -> Result<NodeFrame, Error> {
    let frame = client
        .connection
        .recv()
        .await
        .map_err(CallError::Broken)?
        .ok_or_else(|| broken("the file read ended without its terminal reply"))?;
    serde_json::from_slice(&frame).map_err(|e| broken(&format!("invalid file-read frame: {e}")))
}

fn broken(message: &str) -> Error {
    CallError::Broken(std::io::Error::new(std::io::ErrorKind::InvalidData, message)).into()
}
