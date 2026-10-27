//! Multiplexed watches over the existing OS-protected local endpoint. Each JSON
//! chunk header is followed by one binary frame; cancellation discards only the
//! affected assembly. Network authentication remains in watch_mux.

mod reader;
mod writer;
pub use crate::watch_mux::{MAX_PACKET, QUANTUM, SUBSCRIPTIONS};
pub use reader::{Reader, Received};
pub use writer::{Output, cancel, send, write};

use crate::reassembly::{Buffer, Reservation};
use fastctx_platform::ipc::{FrameReader, FrameWriter};
use fastctx_types::watching::local::Command;
use serde::{Deserialize, Serialize};
use std::{io, time::Duration};

const INFLIGHT_PACKETS: usize = 2 * SUBSCRIPTIONS + 1;
const HEADER: usize = 4096;
pub const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize)]
struct Part {
    subscription: u64,
    number: u64,
    offset: usize,
    total: usize,
    bytes: usize,
    #[serde(default)]
    discard: bool,
}

#[derive(Serialize, Deserialize)]
struct CommandSize {
    command_bytes: usize,
}

pub struct PendingCommand {
    bytes: Vec<u8>,
    _memory: Reservation,
}
impl PendingCommand {
    pub fn new(command: &Command) -> io::Result<Self> {
        let size = crate::watch_queue::size(command);
        if size > MAX_PACKET {
            return Err(io::Error::other("the local watch command exceeds 64 MiB"));
        }
        let memory = Reservation::new(size * 2 + QUANTUM).map_err(io::Error::other)?;
        let bytes = serde_json::to_vec(command)?;
        Ok(Self { bytes, _memory: memory })
    }

    pub async fn send(&self, writer: &mut FrameWriter) -> io::Result<()> {
        tokio::time::timeout(TIMEOUT, async {
            writer.send(&serde_json::to_vec(&CommandSize { command_bytes: self.bytes.len() })?).await?;
            for chunk in self.bytes.chunks(QUANTUM) {
                writer.send(chunk).await?;
            }
            Ok(())
        })
        .await
        .map_err(io::Error::other)?
    }
}

pub struct ReceivedCommand {
    pub command: Command,
    pub memory: Reservation,
}

/// Keep this read in its own task: interruption halfway through a command closes
/// the connection rather than interpreting the next payload frame as a header.
pub async fn command(reader: &mut FrameReader) -> io::Result<Option<ReceivedCommand>> {
    let Some(head) = reader.recv_up_to(HEADER).await? else {
        return Ok(None);
    };
    let size: CommandSize = serde_json::from_slice(&head)?;
    if size.command_bytes == 0 || size.command_bytes > MAX_PACKET {
        return Err(io::Error::other("invalid local watch command size"));
    }
    let mut bytes = Buffer::new(size.command_bytes, size.command_bytes + QUANTUM).map_err(io::Error::other)?;
    tokio::time::timeout(TIMEOUT, async {
        while bytes.len() < size.command_bytes {
            let chunk = reader.recv_up_to(QUANTUM).await?.ok_or(io::ErrorKind::UnexpectedEof)?;
            if chunk.is_empty() {
                return Err(io::Error::other("empty local watch command chunk"));
            }
            bytes.extend(&chunk).map_err(io::Error::other)?;
        }
        let command = serde_json::from_slice(&bytes)?;
        Ok(Some(ReceivedCommand { command, memory: bytes.into_reservation() }))
    })
    .await
    .map_err(io::Error::other)?
}
