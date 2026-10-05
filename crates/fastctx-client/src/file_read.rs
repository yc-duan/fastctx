//! Original file bytes for interfaces, over a local IPC connection or a
//! device's authenticated data stream. Dropping a read cancels it; bounded
//! queues propagate backpressure to the source.

use fastctx_types::file_read::{FileReadComplete, FileReadInfo, FileReadPart, ReadFileRequest};
use fastctx_types::ipc::{ClientFrame, NodeFrame};
use fastctx_types::{Call, MachineName, Reply};

use crate::{CallError, Client, Error, World};

impl World {
    /// Opens a file on `machine` (this node when absent) as the user. A
    /// device has no local node and must name the machine holding the file.
    /// Consume `Reading::next` through its terminal result; bytes handed
    /// out before that are provisional if the read fails or changes.
    pub async fn read_file(&self, machine: Option<MachineName>, request: ReadFileRequest) -> Result<Reading, Error> {
        if let Some(device) = self.device() {
            return Reading::open(Source::Device(device.read_file(machine, request).await?)).await;
        }
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
        Reading::open(Source::Local { client: self, id }).await
    }
}

/// A read's opening metadata and its original byte chunks. No whole-file
/// buffer is retained. `next` is not cancellation-safe: cancel by dropping
/// the entire `Reading`, rather than abandoning a partially received frame.
pub struct Reading {
    source: Source,
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

    async fn open(mut source: Source) -> Result<Self, Error> {
        match source.next().await? {
            fastctx_net::file_read::Part::Opened(info) => {
                if info.offset > info.file.size || info.length > info.file.size - info.offset {
                    return Err(broken("the node selected a range outside the observed file"));
                }
                Ok(Self { source, info, received: 0, complete: None, ended: false })
            }
            fastctx_net::file_read::Part::Finished(Err(error)) => Err(error.into()),
            _ => Err(broken("the file read ended before its opening metadata")),
        }
    }

    async fn receive(&mut self) -> Result<Option<Vec<u8>>, Error> {
        use fastctx_net::file_read::Part;
        match self.source.next().await? {
            Part::Chunk { offset, bytes } => {
                if offset != self.info.offset + self.received
                    || bytes.is_empty()
                    || bytes.len() as u64 > self.info.length - self.received
                {
                    return Err(broken("the file read sent bytes outside the selected range or out of order"));
                }
                self.received += bytes.len() as u64;
                Ok(Some(bytes))
            }
            Part::Finished(result) => {
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

enum Source {
    Local { client: Client, id: u64 },
    Device(crate::device::file_read::Reading),
}

impl Source {
    async fn next(&mut self) -> Result<fastctx_net::file_read::Part, Error> {
        use fastctx_net::file_read::Part;
        let Self::Local { client, id } = self else {
            let Self::Device(reading) = self else { unreachable!() };
            return reading.next().await;
        };
        match receive(client).await? {
            NodeFrame::FileRead { id: answered, part } if answered == *id => match part {
                FileReadPart::Opened { info } => Ok(Part::Opened(info)),
                FileReadPart::Chunk { offset, bytes } => {
                    if bytes == 0 || bytes > fastctx_net::wire::MAX_DATA_MESSAGE as u64 {
                        return Err(broken("the node declared an invalid file-read block size"));
                    }
                    let data = client
                        .connection
                        .recv()
                        .await
                        .map_err(CallError::Broken)?
                        .ok_or_else(|| broken("the file read ended in a binary block"))?;
                    if data.len() as u64 != bytes {
                        return Err(broken("the binary block's length differs from its header"));
                    }
                    Ok(Part::Chunk { offset, bytes: data })
                }
            },
            NodeFrame::Reply { id: answered, reply } if answered == *id => match reply.map_err(CallError::Failed)? {
                Reply::FileRead { result } => Ok(Part::Finished(result)),
                _ => Err(broken("the node answered something other than a file read")),
            },
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
