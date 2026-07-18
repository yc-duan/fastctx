//! Process-stdio adapters whose reader lifetime does not pin the async runtime.

use std::io::Read;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc;

const STDIN_CHUNK_BYTES: usize = 8 * 1024;
const STDIN_QUEUE_DEPTH: usize = 16;

/// Async stdin backed by a detached operating-system reader thread.
///
/// Tokio's global stdin adapter uses the runtime blocking pool. A leaked writer can keep that
/// pool thread blocked forever after the MCP service is cancelled, preventing parent-death
/// shutdown from completing. This adapter deliberately owns an ordinary detached thread instead:
/// EOF still closes the transport normally, while process shutdown never waits for a blocked read.
pub(crate) struct DetachedStdin {
    receiver: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    pending: Vec<u8>,
    pending_offset: usize,
}

impl DetachedStdin {
    /// Starts the sole stdin reader for the stdio MCP transport.
    pub(crate) fn start() -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel(STDIN_QUEUE_DEPTH);
        std::thread::Builder::new()
            .name("fastctx-stdin".to_string())
            .spawn(move || {
                let stdin = std::io::stdin();
                let mut input = stdin.lock();
                loop {
                    let mut bytes = vec![0; STDIN_CHUNK_BYTES];
                    match input.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(read) => {
                            bytes.truncate(read);
                            if sender.blocking_send(Ok(bytes)).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            let _ = sender.blocking_send(Err(error));
                            break;
                        }
                    }
                }
            })
            .map_err(|error| format!("Cannot start the MCP stdin reader: {error}"))?;
        Ok(Self {
            receiver,
            pending: Vec::new(),
            pending_offset: 0,
        })
    }
}

impl AsyncRead for DetachedStdin {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            if self.pending_offset < self.pending.len() {
                let available = &self.pending[self.pending_offset..];
                let copied = available.len().min(output.remaining());
                output.put_slice(&available[..copied]);
                self.pending_offset += copied;
                if self.pending_offset == self.pending.len() {
                    self.pending.clear();
                    self.pending_offset = 0;
                }
                return Poll::Ready(Ok(()));
            }
            match self.receiver.poll_recv(context) {
                Poll::Ready(Some(Ok(bytes))) => {
                    self.pending = bytes;
                    self.pending_offset = 0;
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DetachedStdin;

    #[test]
    fn stdin_adapter_is_send_for_the_rmcp_transport() {
        fn assert_send<T: Send>() {}
        assert_send::<DetachedStdin>();
    }
}
