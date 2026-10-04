//! Async Unix sockets on both Unix and Windows. A direct stream preserves
//! EOF; a byte-forwarding child can keep its stdout open after socket EOF.

use std::io;
use std::path::Path;

#[cfg(unix)]
pub type Stream = tokio::net::UnixStream;
#[cfg(windows)]
pub use windows::Stream;

/// Connects to an existing socket. Resolving its published path also allows
/// Codex's long Unix rendezvous paths to name its short physical sockets.
pub async fn connect(path: &Path) -> io::Result<Stream> {
    #[cfg(unix)]
    let path = tokio::fs::canonicalize(path).await?;
    #[cfg(windows)]
    let path = {
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        tokio::fs::canonicalize(parent).await?.join(name)
    };
    #[cfg(unix)]
    return Stream::connect(path).await;
    #[cfg(windows)]
    return windows::connect(path).await;
}

#[cfg(windows)]
mod windows {
    use std::io::{self, Read, Write};
    use std::net::Shutdown;
    use std::os::windows::io::{AsRawSocket as _, AsSocket, BorrowedSocket};
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::task::{Context, Poll, ready};

    use async_io::Async;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt as _};

    pub struct Stream(Compat<Async<Socket>>);

    pub async fn connect(path: PathBuf) -> io::Result<Stream> {
        let socket = tokio::task::spawn_blocking(move || uds_windows::UnixStream::connect(path))
            .await
            .map_err(|error| io::Error::other(format!("connecting Unix socket: {error}")))??;
        Ok(Stream(Async::new(Socket(socket))?.compat()))
    }

    struct Socket(uds_windows::UnixStream);

    impl AsSocket for Socket {
        fn as_socket(&self) -> BorrowedSocket<'_> {
            // SAFETY: the socket remains owned by self for this borrow.
            unsafe { BorrowedSocket::borrow_raw(self.0.as_raw_socket()) }
        }
    }

    // SAFETY: Read/Write only borrow the socket; neither replaces or closes it.
    unsafe impl async_io::IoSafe for Socket {}

    impl Read for Socket {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0.read(buffer)
        }
    }

    impl Write for Socket {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    impl AsyncRead for Stream {
        fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Stream {
        fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.get_mut().0).poll_write(cx, bytes)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().0).poll_flush(cx)
        }

        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let stream = &mut self.get_mut().0;
            ready!(Pin::new(&mut *stream).poll_flush(cx))?;
            // async-io's poll_close only flushes; Tokio shutdown half-closes.
            stream.get_ref().get_ref().0.shutdown(Shutdown::Write)?;
            Poll::Ready(Ok(()))
        }
    }
}
