//! What a connection runs on, whichever way it went.
//!
//! A machine reaches the hub over TCP, or, when its own line to the hub is
//! down, through a machine that allows direct connections and forwards the
//! hub's bytes as they are (`world-network.md`, 直连). Two machines linked
//! directly speak over TCP too, one of them having accepted the other. All
//! of these end up as one WebSocket on TLS, so the frames and data
//! connections above them do not care which way they went.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use futures_util::{Sink, Stream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// A WebSocket on TLS on whatever the connection runs on: the hub's
/// control and data connections, and direct links between machines.
pub type Socket = WebSocketStream<tokio_rustls::TlsStream<Transport>>;

/// The WebSocket to a forwarding machine that carries the hub's TLS.
pub type ForwardSocket = WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;

/// What the TLS of a connection runs on.
pub enum Transport {
    /// A TCP connection of this machine's own.
    Tcp(TcpStream),
    /// The bytes of a connection to the hub, carried by a machine that
    /// forwards them, unread, to the hub.
    Forwarded(Box<WsIo>),
}

impl AsyncRead for Transport {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Transport::Tcp(tcp) => Pin::new(tcp).poll_read(cx, buf),
            Transport::Forwarded(forwarded) => Pin::new(forwarded.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Transport {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Transport::Tcp(tcp) => Pin::new(tcp).poll_write(cx, buf),
            Transport::Forwarded(forwarded) => Pin::new(forwarded.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Transport::Tcp(tcp) => Pin::new(tcp).poll_flush(cx),
            Transport::Forwarded(forwarded) => Pin::new(forwarded.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Transport::Tcp(tcp) => Pin::new(tcp).poll_shutdown(cx),
            Transport::Forwarded(forwarded) => Pin::new(forwarded.as_mut()).poll_shutdown(cx),
        }
    }
}

/// A byte stream carried in the binary messages of a WebSocket: what this
/// machine writes goes as one message per write, and what comes back is
/// read message by message.
pub struct WsIo {
    socket: ForwardSocket,
    pending: bytes::Bytes,
    ended: bool,
}

impl WsIo {
    pub fn new(socket: ForwardSocket) -> Self {
        Self { socket, pending: bytes::Bytes::new(), ended: false }
    }
}

fn broken(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, error.to_string())
}

impl AsyncRead for WsIo {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if !this.pending.is_empty() {
                let here = this.pending.len().min(buf.remaining());
                buf.put_slice(&this.pending.split_to(here));
                return Poll::Ready(Ok(()));
            }
            if this.ended {
                return Poll::Ready(Ok(()));
            }
            match ready!(Pin::new(&mut this.socket).poll_next(cx)) {
                None | Some(Ok(WsMessage::Close(_))) => this.ended = true,
                Some(Ok(WsMessage::Binary(bytes))) => this.pending = bytes,
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
                Some(Ok(WsMessage::Text(_))) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "the forwarding machine sent text where bytes were due",
                    )));
                }
                Some(Err(error)) => return Poll::Ready(Err(broken(error))),
            }
        }
    }
}

impl AsyncWrite for WsIo {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        ready!(Pin::new(&mut this.socket).poll_ready(cx)).map_err(broken)?;
        Pin::new(&mut this.socket).start_send(WsMessage::Binary(bytes::Bytes::copy_from_slice(buf))).map_err(broken)?;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().socket).poll_flush(cx).map_err(broken)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().socket).poll_close(cx).map_err(broken)
    }
}
