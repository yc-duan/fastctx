//! Reaching the hub: TCP, TLS on the pinned certificate, and a WebSocket
//! on one of the hub's two paths.

use std::net::SocketAddr;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::trust::CertPin;
use crate::wire::{Down, EXPORTER_LABEL, Up};

pub type HubSocket = WebSocketStream<TlsStream<TcpStream>>;

/// A connection to the hub, before anything has been said on it.
pub struct Dialed {
    pub socket: HubSocket,
    /// The TLS exporter of this connection, which the machine signs to
    /// prove it is the one on it.
    pub binding: [u8; 32],
    pub address: SocketAddr,
}

/// Connects to the first of the hub's addresses that answers.
pub async fn dial(hub: &[SocketAddr], pin: CertPin, path: &str, max_message: usize) -> Result<Dialed, String> {
    let mut failures = Vec::new();
    for address in hub {
        match dial_one(*address, pin, path, max_message).await {
            Ok(dialed) => return Ok(dialed),
            Err(error) => failures.push(format!("{address}: {error}")),
        }
    }
    if failures.is_empty() {
        return Err("no address for the hub is known".to_string());
    }
    Err(failures.join("; "))
}

async fn dial_one(address: SocketAddr, pin: CertPin, path: &str, max_message: usize) -> Result<Dialed, String> {
    let tcp = TcpStream::connect(address).await.map_err(|error| format!("connecting: {error}"))?;
    configure(&tcp);
    let tls = TlsConnector::from(crate::tls::client_config(pin))
        .connect(rustls::pki_types::ServerName::IpAddress(address.ip().into()), tcp)
        .await
        .map_err(|error| format!("TLS: {error}"))?;
    let mut binding = [0_u8; 32];
    tls.get_ref()
        .1
        .export_keying_material(&mut binding, EXPORTER_LABEL, None)
        .map_err(|error| format!("TLS exporter: {error}"))?;
    let url = format!("ws://{address}{path}");
    let (socket, _) = tokio_tungstenite::client_async_with_config(url, tls, Some(config(max_message)))
        .await
        .map_err(|error| format!("WebSocket: {error}"))?;
    Ok(Dialed { socket, binding, address })
}

pub fn config(max_message: usize) -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(max_message)).max_frame_size(Some(max_message))
}

/// Keepalive lets the operating system notice a peer that vanished
/// without closing; no delay, since frames are small and each one is
/// awaited.
pub fn configure(tcp: &TcpStream) {
    let socket = socket2::SockRef::from(tcp);
    let _ = socket.set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(std::time::Duration::from_secs(30)));
    let _ = socket.set_tcp_nodelay(true);
}

/// What went wrong on a connection, in a few words: a connection the other
/// side dropped without closing it is "cut", not a TLS error.
pub fn describe(error: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
            ) =>
        {
            "the connection was cut".to_string()
        }
        Error::ConnectionClosed | Error::AlreadyClosed => "the connection was closed".to_string(),
        other => other.to_string(),
    }
}

/// Sends one frame.
pub async fn send<S>(socket: &mut S, frame: &Up) -> Result<(), String>
where
    S: futures_util::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    socket.send(WsMessage::Binary(crate::wire::encode(frame).into())).await.map_err(|error| error.to_string())
}

/// Reads the next frame; `None` when the hub closed the connection.
pub async fn recv<S>(socket: &mut S) -> Result<Option<Down>, String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match socket.next().await {
            None | Some(Ok(WsMessage::Close(_))) => return Ok(None),
            Some(Ok(WsMessage::Binary(bytes))) => {
                return serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|error| format!("a frame from the hub: {error}"));
            }
            Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
            Some(Ok(WsMessage::Text(_))) => {
                return Err("the hub sent text, not a frame".to_string());
            }
            Some(Err(error)) => return Err(describe(&error)),
        }
    }
}
