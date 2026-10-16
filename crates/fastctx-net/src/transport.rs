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

/// When the bytes of `socket` last moved, each way (`crate::pulse`).
pub fn pulse_of(socket: &Socket) -> std::sync::Arc<crate::pulse::Pulse> {
    socket.get_ref().get_ref().0.pulse()
}

/// The WebSocket to a forwarding machine that carries the hub's TLS.
pub type ForwardSocket = WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;

/// What the TLS of a connection runs on, keeping when its bytes last moved
/// (`crate::pulse`).
pub struct Transport(crate::pulse::Pulsing<Way>);

impl Transport {
    /// A TCP connection of this machine's own.
    pub fn tcp(tcp: TcpStream) -> Self {
        Self(crate::pulse::Pulsing::new(Way::Tcp(tcp)))
    }

    /// The bytes of a connection to the hub, carried by a machine that
    /// forwards them, unread, to the hub.
    pub fn forwarded(carried: WsIo) -> Self {
        Self(crate::pulse::Pulsing::new(Way::Forwarded(Box::new(carried))))
    }

    pub fn pulse(&self) -> std::sync::Arc<crate::pulse::Pulse> {
        self.0.pulse()
    }
}

/// The ways a connection runs.
pub enum Way {
    Tcp(TcpStream),
    Forwarded(Box<WsIo>),
}

impl AsyncRead for Transport {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Transport {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}

impl AsyncRead for Way {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Way::Tcp(tcp) => Pin::new(tcp).poll_read(cx, buf),
            Way::Forwarded(forwarded) => Pin::new(forwarded.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Way {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Way::Tcp(tcp) => Pin::new(tcp).poll_write(cx, buf),
            Way::Forwarded(forwarded) => Pin::new(forwarded.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Way::Tcp(tcp) => Pin::new(tcp).poll_flush(cx),
            Way::Forwarded(forwarded) => Pin::new(forwarded.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Way::Tcp(tcp) => Pin::new(tcp).poll_shutdown(cx),
            Way::Forwarded(forwarded) => Pin::new(forwarded.as_mut()).poll_shutdown(cx),
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
