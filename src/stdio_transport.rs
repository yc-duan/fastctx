//! Process-stdio adapters whose reader lifetime does not pin the async runtime.

use std::io::Read;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

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
    eof: CancellationToken,
    read_error: watch::Receiver<Option<String>>,
}

impl DetachedStdin {
    /// Starts the sole stdin reader for the stdio MCP transport.
    pub(crate) fn start() -> Result<Self, String> {
        Self::spawn_reader(|sender, read_error_sender| {
            let stdin = std::io::stdin();
            forward_reader(stdin.lock(), sender, read_error_sender);
        })
    }

    fn spawn_reader(
        reader: impl FnOnce(mpsc::Sender<std::io::Result<Vec<u8>>>, watch::Sender<Option<String>>)
        + Send
        + 'static,
    ) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel(STDIN_QUEUE_DEPTH);
        let (read_error_sender, read_error) = watch::channel(None);
        let eof = CancellationToken::new();
        std::thread::Builder::new()
            .name("fastctx-stdin".to_string())
            .spawn(move || reader(sender, read_error_sender))
            .map_err(|error| format!("Cannot start the MCP stdin reader: {error}"))?;
        Ok(Self {
            receiver,
            pending: Vec::new(),
            pending_offset: 0,
            eof,
            read_error,
        })
    }

    /// Signals clean EOF after every byte already read from stdin has reached the transport.
    pub(crate) fn eof_token(&self) -> CancellationToken {
        self.eof.clone()
    }

    /// Reports an operating-system read failure independently of rmcp's EOF-shaped transport API.
    pub(crate) fn read_error_receiver(&self) -> watch::Receiver<Option<String>> {
        self.read_error.clone()
    }

    #[cfg(test)]
    pub(crate) fn start_with_reader(reader: impl Read + Send + 'static) -> Self {
        Self::spawn_reader(move |sender, read_error_sender| {
            forward_reader(reader, sender, read_error_sender);
        })
        .expect("test stdin reader thread must start")
    }
}

fn forward_reader(
    mut input: impl Read,
    sender: mpsc::Sender<std::io::Result<Vec<u8>>>,
    read_error_sender: watch::Sender<Option<String>>,
) {
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
                read_error_sender.send_replace(Some(format!("Cannot read MCP stdin: {error}")));
                let _ = sender.blocking_send(Err(error));
                break;
            }
        }
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
                Poll::Ready(None) => {
                    self.eof.cancel();
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DetachedStdin;
    use tokio::io::AsyncReadExt;
    use tokio::sync::{mpsc, watch};
    use tokio_util::sync::CancellationToken;

    fn adapter(
        receiver: mpsc::Receiver<std::io::Result<Vec<u8>>>,
        eof: CancellationToken,
        read_error: watch::Receiver<Option<String>>,
    ) -> DetachedStdin {
        DetachedStdin {
            receiver,
            pending: Vec::new(),
            pending_offset: 0,
            eof,
            read_error,
        }
    }

    #[test]
    fn stdin_adapter_is_send_for_the_rmcp_transport() {
        fn assert_send<T: Send>() {}
        assert_send::<DetachedStdin>();
    }

    #[tokio::test]
    async fn eof_signal_follows_all_buffered_input() {
        let (sender, receiver) = mpsc::channel(1);
        sender.send(Ok(b"complete frame".to_vec())).await.unwrap();
        drop(sender);
        let eof = CancellationToken::new();
        let (_read_error_sender, read_error) = watch::channel(None);
        let mut input = adapter(receiver, eof.clone(), read_error);

        assert!(!eof.is_cancelled());
        let mut prefix = [0; 4];
        input.read_exact(&mut prefix).await.unwrap();
        assert_eq!(&prefix, b"comp");
        assert!(!eof.is_cancelled());

        let mut suffix = Vec::new();
        input.read_to_end(&mut suffix).await.unwrap();

        assert_eq!(suffix, b"lete frame");
        assert!(eof.is_cancelled());
    }

    #[tokio::test]
    async fn read_error_is_not_reported_as_clean_eof() {
        let (sender, receiver) = mpsc::channel(1);
        sender
            .send(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "injected stdin failure",
            )))
            .await
            .unwrap();
        drop(sender);
        let eof = CancellationToken::new();
        let (_read_error_sender, read_error) = watch::channel(None);
        let mut input = adapter(receiver, eof.clone(), read_error);

        let error = input.read_to_end(&mut Vec::new()).await.unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert!(!eof.is_cancelled());
    }
}
