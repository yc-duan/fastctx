//! Reaching the hub: TCP, TLS on the pinned certificate, and a WebSocket
//! on one of the hub's two paths. Relays forward the hub's port as it is,
//! so the same certificate answers through them.

use std::net::SocketAddr;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::ids::CertPin;
use fastctx_types::world::Route;

use crate::wire::{Down, EXPORTER_LABEL, SILENCE, Up};

pub type HubSocket = WebSocketStream<TlsStream<TcpStream>>;

/// A connection to the hub, before anything has been said on it.
pub struct Dialed {
    pub socket: HubSocket,
    /// The TLS exporter of this connection, which the machine signs to
    /// prove it is the one on it.
    pub binding: [u8; 32],
    pub address: SocketAddr,
    /// The way the connection left this machine.
    pub route: Route,
    /// The certificate the server presented, as DER.
    pub certificate: Vec<u8>,
}

/// One way out of this machine (`route.rs`).
enum Way {
    /// Pinned to the physical network card.
    #[cfg(not(target_os = "android"))]
    Pinned(crate::route::Interface),
    #[cfg(target_os = "android")]
    Network(crate::android::Network),
    Proxy(crate::proxy::Proxy),
    /// Wherever the system's routes send it.
    System,
}

impl Way {
    fn route(&self) -> Route {
        match self {
            #[cfg(not(target_os = "android"))]
            Way::Pinned(interface) => Route::Card { name: interface.name.clone() },
            #[cfg(target_os = "android")]
            Way::Network(network) => Route::Card { name: network.name.clone() },
            Way::Proxy(proxy) => Route::Proxy { proxy: proxy.to_string() },
            Way::System => Route::System,
        }
    }
}

impl std::fmt::Display for Way {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.route().fmt(f)
    }
}

/// The ways to try for `address`, in order: the physical card, then the
/// system's proxy and its routes. Nothing captures a loopback
/// address, so it is reached plainly.
#[cfg(not(target_os = "android"))]
fn ways(address: SocketAddr) -> (Vec<Way>, Option<String>) {
    if address.ip().is_loopback() {
        return (vec![Way::System], None);
    }
    let mut ways = Vec::new();
    let mut note = None;
    match crate::route::interfaces() {
        Ok(interfaces) => match crate::route::physical(&interfaces) {
            Some(interface) => ways.push(Way::Pinned(interface.clone())),
            None => note = Some("no physical network card is up with an address".to_string()),
        },
        Err(error) => note = Some(error),
    }
    if let Some(proxy) = crate::proxy::system(address) {
        ways.push(Way::Proxy(proxy));
    }
    ways.push(Way::System);
    (ways, note)
}

#[cfg(target_os = "android")]
fn ways(address: SocketAddr) -> (Vec<Way>, Option<String>) {
    if address.ip().is_loopback() {
        return (vec![Way::System], None);
    }
    let choices = match crate::android::choices(address) {
        Ok(choices) => choices,
        Err(error) => return (vec![], Some(error)),
    };
    let mut ways: Vec<_> = choices.networks.into_iter().map(Way::Network).collect();
    let mut unsupported = Vec::new();
    for proxy in choices.proxies {
        match proxy {
            crate::android::Proxy::Http { host, port } => ways.push(Way::Proxy(crate::proxy::Proxy {
                host,
                port,
                credentials: None,
                source: "Android ProxySelector".into(),
            })),
            crate::android::Proxy::Direct => ways.push(Way::System),
            crate::android::Proxy::Unsupported { description } => unsupported.push(description),
        }
    }
    if !ways.iter().any(|way| matches!(way, Way::System)) {
        ways.push(Way::System);
    }
    (ways, (!unsupported.is_empty()).then(|| format!("unsupported Android proxies: {}", unsupported.join(", "))))
}

impl Way {
    fn kind(&self) -> WayKind {
        match self {
            #[cfg(not(target_os = "android"))]
            Self::Pinned(_) => WayKind::Pinned,
            #[cfg(target_os = "android")]
            Self::Network(_) => WayKind::Pinned,
            Self::Proxy(_) => WayKind::Proxy,
            Self::System => WayKind::System,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WayKind {
    Pinned,
    Proxy,
    System,
}

/// The way that last reached each address is tried first next time. A
/// dead proxy must not delay every new data connection after the system
/// route has already succeeded.
static LAST_WAY: std::sync::Mutex<std::collections::BTreeMap<SocketAddr, WayKind>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// A formerly successful interface or proxy may no longer exist.
pub(crate) fn network_changed() {
    LAST_WAY.lock().expect("never poisoned").clear();
}

/// Connects to the first of the hub's addresses that answers, each by the
/// first way that reaches it. Without `pin`, any certificate is taken: only
/// for the first contact of a password login, which proves the hub itself.
pub async fn dial(hub: &[SocketAddr], pin: Option<CertPin>, path: &str, max_message: usize) -> Result<Dialed, String> {
    #[cfg(target_os = "android")]
    {
        let mut changes = crate::android::changes();
        loop {
            tokio::select! {
                result = dial_routes(hub, pin, path, max_message) => return result,
                changed = changes.changed() => changed.map_err(|error| error.to_string())?,
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    dial_routes(hub, pin, path, max_message).await
}

async fn dial_routes(
    hub: &[SocketAddr],
    pin: Option<CertPin>,
    path: &str,
    max_message: usize,
) -> Result<Dialed, String> {
    let mut failures = Vec::new();
    for address in hub {
        let (mut ways, note) = ways(*address);
        if let Some(previous) = LAST_WAY.lock().expect("never poisoned").get(address).copied() {
            // A stable sort keeps the rest in order.
            ways.sort_by_key(|way| way.kind() != previous);
        }
        if let Some(note) = note {
            failures.push(format!("{address}: {note}"));
        }
        for way in ways {
            // Each way gets the time the World gives any silent peer, so a
            // way that swallows packets does not use up the next one's.
            match tokio::time::timeout(SILENCE, dial_one(*address, &way, pin, path, max_message)).await {
                Ok(Ok(dialed)) => {
                    LAST_WAY.lock().expect("never poisoned").insert(*address, way.kind());
                    return Ok(dialed);
                }
                Ok(Err(error)) => failures.push(format!("{address} {way}: {error}")),
                Err(_) => failures.push(format!("{address} {way}: no answer within {} s", SILENCE.as_secs())),
            }
        }
    }
    if failures.is_empty() {
        return Err("no address for the hub is known".to_string());
    }
    Err(failures.join("; "))
}

async fn dial_one(
    address: SocketAddr,
    way: &Way,
    pin: Option<CertPin>,
    path: &str,
    max_message: usize,
) -> Result<Dialed, String> {
    let tcp = match way {
        #[cfg(not(target_os = "android"))]
        Way::Pinned(interface) => crate::route::pinned_socket(interface, address)?
            .connect(address)
            .await
            .map_err(|error| format!("connecting: {error}"))?,
        #[cfg(target_os = "android")]
        Way::Network(network) => crate::android::socket(network, address)?
            .connect(address)
            .await
            .map_err(|error| format!("connecting: {error}"))?,
        Way::Proxy(proxy) => crate::proxy::connect(proxy, address).await?,
        Way::System => TcpStream::connect(address).await.map_err(|error| format!("connecting: {error}"))?,
    };
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
    let certificate = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .map(|certificate| certificate.as_ref().to_vec())
        .ok_or_else(|| "the server presented no certificate".to_string())?;
    let url = format!("ws://{address}{path}");
    let (socket, _) = tokio_tungstenite::client_async_with_config(url, tls, Some(config(max_message)))
        .await
        .map_err(|error| format!("WebSocket: {error}"))?;
    let route = if address.ip().is_loopback() { Route::Local } else { way.route() };
    Ok(Dialed { socket, binding, address, route, certificate })
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

/// How long to wait before dialing again after `failures` failures in a
/// row: the first retry is at once; after that a random wait up to a ceiling
/// that doubles with each failure, to a minute, so machines that lost their
/// upstream together do not all return in the same instant.
pub fn backoff(failures: u32) -> std::time::Duration {
    if failures <= 1 {
        return std::time::Duration::ZERO;
    }
    let ceiling_ms = 1000_u64.saturating_mul(1 << (failures - 2).min(6)).min(60_000);
    let random = u64::from_le_bytes(crate::sign::random::<8>().unwrap_or([0; 8]));
    std::time::Duration::from_millis(250 + random % ceiling_ms.saturating_sub(250).max(1))
}

/// Asks the upstream at `addresses` whether it takes sessions now
/// (`wire::Intent::Probe`).
pub async fn probe(addresses: &[SocketAddr], pin: CertPin) -> Result<(), String> {
    let mut socket =
        dial(addresses, Some(pin), crate::wire::CONTROL_PATH, crate::wire::MAX_CONTROL_FRAME).await?.socket;
    let hello = Up::Hello {
        protocol: crate::wire::PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        intent: crate::wire::Intent::Probe,
    };
    send(&mut socket, &hello).await?;
    let answer = tokio::time::timeout(SILENCE, recv(&mut socket)).await;
    let _ = socket.close(None).await;
    match answer {
        Ok(Ok(Some(Down::Ready))) => Ok(()),
        Ok(Ok(Some(Down::Refused { reason }))) => Err(reason.to_string()),
        Ok(Ok(_)) => Err("it answered something other than whether it is ready".to_string()),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(format!("no answer within {} s", SILENCE.as_secs())),
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
