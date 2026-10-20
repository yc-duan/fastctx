//! Reaching the hub, or another machine directly: TCP by the first way out
//! of this machine that works, TLS, and a WebSocket on one of the paths.
//!
//! The hub is reached on its pinned certificate, by this machine's own line
//! or, when that is down, through a machine that allows direct connections
//! and forwards the hub's bytes as they are: the TLS with the hub runs
//! inside the forwarding machine's connection, so it can neither read nor
//! change it (`world-network.md`, 直连). A machine reached directly shows
//! any certificate; who it is, it proves with its key on top
//! (`crate::peer`).

use std::net::SocketAddr;

use fastctx_types::MachineName;
use fastctx_types::world::Route;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::ids::{CertPin, HubId};
use crate::transport::{Socket, Transport, WsIo};
use crate::wire::{Down, EXPORTER_LABEL, SILENCE, Up};

pub type HubSocket = Socket;

/// A connection, before anything has been said on it.
pub struct Dialed {
    pub socket: HubSocket,
    /// The TLS exporter of this connection, which the machine signs to
    /// prove it is the one on it.
    pub binding: [u8; 32],
    /// The address this machine connected to: the hub's, a machine's it
    /// links with directly, or the machine's that forwards to the hub.
    pub address: SocketAddr,
    /// The way the connection left this machine.
    pub route: Route,
    /// The certificate the server presented, as DER.
    pub certificate: Vec<u8>,
}

/// What a relay named by a code is called until a view names its machine:
/// it is said only where a way is told, as in `through relay`.
pub fn relay_name() -> MachineName {
    MachineName::try_from("relay".to_string()).expect("a valid machine name")
}

/// A machine that allows direct connections, through which others reach
/// the hub when their own line is down.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Forwarder {
    pub name: MachineName,
    pub at: Vec<SocketAddr>,
}

/// A way to the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Upstream {
    /// This machine's own line to the hub's addresses.
    Hub(Vec<SocketAddr>),
    /// Through a machine that forwards the hub's bytes.
    Through(Forwarder),
}

impl Upstream {
    /// The machine it goes through, if any.
    pub fn through(&self) -> Option<&MachineName> {
        match self {
            Upstream::Hub(_) => None,
            Upstream::Through(forwarder) => Some(&forwarder.name),
        }
    }
}

impl std::fmt::Display for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Upstream::Hub(_) => f.write_str("its own line"),
            Upstream::Through(forwarder) => write!(f, "through {}", forwarder.name),
        }
    }
}

/// One way out of this machine (`route.rs`).
enum Exit {
    /// Pinned to the physical network card.
    #[cfg(not(target_os = "android"))]
    Pinned(crate::route::Interface),
    #[cfg(target_os = "android")]
    Network(crate::android::Network),
    Proxy(crate::proxy::Proxy),
    /// Wherever the system's routes send it.
    System,
}

impl Exit {
    fn route(&self) -> Route {
        match self {
            #[cfg(not(target_os = "android"))]
            Exit::Pinned(interface) => Route::Card { name: interface.name.clone() },
            #[cfg(target_os = "android")]
            Exit::Network(network) => Route::Card { name: network.name.clone() },
            Exit::Proxy(proxy) => Route::Proxy { proxy: proxy.to_string() },
            Exit::System => Route::System,
        }
    }

    fn kind(&self) -> ExitKind {
        match self {
            #[cfg(not(target_os = "android"))]
            Self::Pinned(_) => ExitKind::Pinned,
            #[cfg(target_os = "android")]
            Self::Network(_) => ExitKind::Pinned,
            Self::Proxy(_) => ExitKind::Proxy,
            Self::System => ExitKind::System,
        }
    }
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.route().fmt(f)
    }
}

/// The ways out to try for `address`, in order: the physical card, then the
/// system's proxy and its routes. Nothing captures a loopback address, so it
/// is reached plainly.
#[cfg(not(target_os = "android"))]
fn exits(address: SocketAddr) -> (Vec<Exit>, Option<String>) {
    if address.ip().is_loopback() {
        return (vec![Exit::System], None);
    }
    let mut exits = Vec::new();
    let mut note = None;
    match crate::route::interfaces() {
        Ok(interfaces) => match crate::route::physical(&interfaces) {
            Some(interface) => exits.push(Exit::Pinned(interface.clone())),
            None => note = Some("no physical network card is up with an address".to_string()),
        },
        Err(error) => note = Some(error),
    }
    if let Some(proxy) = crate::proxy::system(address) {
        exits.push(Exit::Proxy(proxy));
    }
    exits.push(Exit::System);
    (exits, note)
}

#[cfg(target_os = "android")]
fn exits(address: SocketAddr) -> (Vec<Exit>, Option<String>) {
    if address.ip().is_loopback() {
        return (vec![Exit::System], None);
    }
    let choices = match crate::android::choices(address) {
        Ok(choices) => choices,
        Err(error) => return (vec![], Some(error)),
    };
    let mut exits: Vec<_> = choices.networks.into_iter().map(Exit::Network).collect();
    let mut unsupported = Vec::new();
    for proxy in choices.proxies {
        match proxy {
            crate::android::Proxy::Http { host, port } => exits.push(Exit::Proxy(crate::proxy::Proxy {
                host,
                port,
                credentials: None,
                source: "Android ProxySelector".into(),
            })),
            crate::android::Proxy::Direct => exits.push(Exit::System),
            crate::android::Proxy::Unsupported { description } => unsupported.push(description),
        }
    }
    if !exits.iter().any(|exit| matches!(exit, Exit::System)) {
        exits.push(Exit::System);
    }
    (exits, (!unsupported.is_empty()).then(|| format!("unsupported Android proxies: {}", unsupported.join(", "))))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExitKind {
    Pinned,
    Proxy,
    System,
}

/// The way out that last reached each address is tried first next time. A
/// dead proxy must not delay every new data connection after the system
/// route has already succeeded.
static LAST_EXIT: std::sync::Mutex<std::collections::BTreeMap<SocketAddr, ExitKind>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// A formerly successful interface or proxy may no longer exist.
pub(crate) fn network_changed() {
    LAST_EXIT.lock().expect("never poisoned").clear();
}

/// Connects to the first of the hub's addresses that answers, each by the
/// first way out that reaches it. Without `pin`, any certificate is taken:
/// only for the first contact of a password login, which proves the hub
/// itself.
pub async fn dial(hub: &[SocketAddr], pin: Option<CertPin>, path: &str, max_message: usize) -> Result<Dialed, String> {
    let (hub, path) = (hub.to_vec(), path.to_string());
    on_own_task(async move {
        watching_android(|| {
            reach(&hub, |tcp, address| tls_websocket(Transport::tcp(tcp), address, pin, &path, max_message))
        })
        .await
    })
    .await
}

/// Runs a dial on a task of its own and waits for it. A dial nests many
/// futures, TCP by each way out, TLS, a WebSocket handshake, and inside a
/// forward a second TLS and WebSocket; polling them takes hundreds of
/// kilobytes of stack in a build without optimizations. A task of its own
/// starts at the top of a worker thread's stack instead of below whatever
/// called it, such as the node's handling of a local call. Dropping the
/// wait ends the dial.
async fn on_own_task<F>(dialing: F) -> Result<Dialed, String>
where
    F: std::future::Future<Output = Result<Dialed, String>> + Send + 'static,
{
    struct Abort(tokio::task::AbortHandle);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let task = tokio::spawn(dialing);
    let _abort = Abort(task.abort_handle());
    match task.await {
        Ok(dialed) => dialed,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => Err(format!("the dial was stopped: {error}")),
    }
}

/// Connects to the hub `hub` by `upstream`: its own line, or through the
/// machine that forwards for it, on the hub's certificate `pin`.
pub async fn dial_upstream(
    upstream: &Upstream,
    hub: &HubId,
    pin: CertPin,
    path: &str,
    max_message: usize,
) -> Result<Dialed, String> {
    match upstream {
        Upstream::Hub(addresses) => dial(addresses, Some(pin), path, max_message).await,
        Upstream::Through(forwarder) => {
            let forward = format!("{}/{hub}", crate::peer::FORWARD_PATH);
            let (at, path) = (forwarder.at.clone(), path.to_string());
            on_own_task(async move {
                watching_android(|| {
                    reach(&at, |tcp, address| {
                        let (forward, path) = (forward.clone(), path.clone());
                        async move {
                            // Any certificate is taken from the forwarding
                            // machine: the hub's own TLS inside is pinned,
                            // and nothing else is said to the forwarding
                            // machine.
                            let (outer, _, _) = tls(tcp, address, None).await?;
                            let carried = config(crate::peer::MAX_FORWARD_MESSAGE);
                            let (socket, _) = tokio_tungstenite::client_async_with_config(
                                format!("ws://{address}{forward}"),
                                outer,
                                Some(carried),
                            )
                            .await
                            .map_err(|error| format!("asking it to forward: {error}"))?;
                            let inner = Transport::forwarded(WsIo::new(socket));
                            tls_websocket(inner, address, Some(pin), &path, max_message)
                                .await
                                .map_err(|error| format!("the hub through it: {error}"))
                        }
                    })
                })
                .await
            })
            .await
        }
    }
}

/// Connects to a machine at `at` that allows direct connections. Its
/// certificate is whatever it shows; it proves who it is with its key.
pub async fn dial_peer(at: &[SocketAddr], path: &str, max_message: usize) -> Result<Dialed, String> {
    let (at, path) = (at.to_vec(), path.to_string());
    on_own_task(async move {
        watching_android(|| {
            reach(&at, |tcp, address| tls_websocket(Transport::tcp(tcp), address, None, &path, max_message))
        })
        .await
    })
    .await
}

/// Dials, starting again with the networks Android has when they change
/// midway.
async fn watching_android<F, Fut>(dialing: F) -> Result<Dialed, String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<Dialed, String>>,
{
    #[cfg(target_os = "android")]
    {
        let mut changes = crate::android::changes();
        loop {
            tokio::select! {
                result = dialing() => return result,
                changed = changes.changed() => changed.map_err(|error| error.to_string())?,
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    dialing().await
}

/// Tries each address by each way out of this machine, until `attempt`
/// succeeds on a TCP connection. Each try gets the time the World gives any
/// silent peer, so a way that swallows packets does not use up the next
/// one's.
async fn reach<F, Fut>(addresses: &[SocketAddr], attempt: F) -> Result<Dialed, String>
where
    F: Fn(TcpStream, SocketAddr) -> Fut,
    Fut: std::future::Future<Output = Result<Dialed, String>>,
{
    let mut failures = Vec::new();
    for address in addresses {
        let (mut exits, note) = exits(*address);
        if let Some(previous) = LAST_EXIT.lock().expect("never poisoned").get(address).copied() {
            // A stable sort keeps the rest in order.
            exits.sort_by_key(|exit| exit.kind() != previous);
        }
        if let Some(note) = note {
            failures.push(format!("{address}: {note}"));
        }
        for exit in exits {
            // Silence before a connection is a way closed on the network;
            // after one, something on the way or at the other end that
            // takes connections and says nothing.
            let reached = std::sync::atomic::AtomicBool::new(false);
            let tried = async {
                let tcp = connect(&exit, *address).await?;
                reached.store(true, std::sync::atomic::Ordering::Relaxed);
                configure(&tcp);
                let mut dialed = attempt(tcp, *address).await?;
                dialed.route = if address.ip().is_loopback() { Route::Local } else { exit.route() };
                Ok::<_, String>(dialed)
            };
            match tokio::time::timeout(SILENCE, tried).await {
                Ok(Ok(dialed)) => {
                    LAST_EXIT.lock().expect("never poisoned").insert(*address, exit.kind());
                    return Ok(dialed);
                }
                Ok(Err(error)) => failures.push(format!("{address} {exit}: {error}")),
                Err(_) if reached.load(std::sync::atomic::Ordering::Relaxed) => failures.push(format!(
                    "{address} {exit}: connected, then no answer within {} s",
                    SILENCE.as_secs()
                )),
                Err(_) => failures.push(format!("{address} {exit}: no connection within {} s", SILENCE.as_secs())),
            }
        }
    }
    if failures.is_empty() {
        return Err("no address is known".to_string());
    }
    Err(failures.join("; "))
}

async fn connect(exit: &Exit, address: SocketAddr) -> Result<TcpStream, String> {
    match exit {
        #[cfg(not(target_os = "android"))]
        Exit::Pinned(interface) => crate::route::pinned_socket(interface, address)?
            .connect(address)
            .await
            .map_err(|error| format!("connecting: {error}")),
        #[cfg(target_os = "android")]
        Exit::Network(network) => crate::android::socket(network, address)?
            .connect(address)
            .await
            .map_err(|error| format!("connecting: {error}")),
        Exit::Proxy(proxy) => crate::proxy::connect(proxy, address).await,
        Exit::System => TcpStream::connect(address).await.map_err(|error| format!("connecting: {error}")),
    }
}

/// Connects to one of `addresses` by this machine's own ways out: how a
/// machine that forwards another's connection reaches the hub.
pub(crate) async fn connect_tcp(addresses: &[SocketAddr]) -> Result<TcpStream, String> {
    let mut failures = Vec::new();
    for address in addresses {
        let (exits, note) = exits(*address);
        if let Some(note) = note {
            failures.push(format!("{address}: {note}"));
        }
        for exit in exits {
            match tokio::time::timeout(SILENCE, connect(&exit, *address)).await {
                Ok(Ok(tcp)) => {
                    configure(&tcp);
                    return Ok(tcp);
                }
                Ok(Err(error)) => failures.push(format!("{address} {exit}: {error}")),
                Err(_) => failures.push(format!("{address} {exit}: no connection within {} s", SILENCE.as_secs())),
            }
        }
    }
    if failures.is_empty() {
        return Err("no address of the hub is known".to_string());
    }
    Err(failures.join("; "))
}

/// The client side of TLS on `io`, with its exporter and the certificate
/// the server showed.
async fn tls<IO>(
    io: IO,
    address: SocketAddr,
    pin: Option<CertPin>,
) -> Result<(tokio_rustls::client::TlsStream<IO>, [u8; 32], Vec<u8>), String>
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let tls = TlsConnector::from(crate::tls::client_config(pin))
        .connect(rustls::pki_types::ServerName::IpAddress(address.ip().into()), io)
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
    Ok((tls, binding, certificate))
}

async fn tls_websocket(
    transport: Transport,
    address: SocketAddr,
    pin: Option<CertPin>,
    path: &str,
    max_message: usize,
) -> Result<Dialed, String> {
    let (tls, binding, certificate) = tls(transport, address, pin).await?;
    let url = format!("ws://{address}{path}");
    let (socket, _) = tokio_tungstenite::client_async_with_config(
        url,
        tokio_rustls::TlsStream::Client(tls),
        Some(config(max_message)),
    )
    .await
    .map_err(|error| format!("WebSocket: {error}"))?;
    Ok(Dialed { socket, binding, address, route: Route::System, certificate })
}

pub fn config(max_message: usize) -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(max_message)).max_frame_size(Some(max_message))
}

/// Keepalive lets the operating system notice a peer that vanished
/// without closing, on a connection nothing has been said on for a while;
/// every long-lived connection also carries its own heartbeat, so this
/// rarely comes into play. Its interval is set: left out, Windows probes
/// back to back, and on a path that drops keepalive probes takes a
/// connection idle for 30 s for dead. No delay, since frames are small and
/// each one is awaited.
pub fn configure(tcp: &TcpStream) {
    let socket = socket2::SockRef::from(tcp);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(std::time::Duration::from_secs(30))
        .with_interval(std::time::Duration::from_secs(10));
    let _ = socket.set_tcp_keepalive(&keepalive);
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

/// Asks the hub `hub` by `upstream` whether it takes sessions now
/// (`wire::Intent::Probe`).
pub async fn probe(upstream: &Upstream, hub: &HubId, pin: CertPin) -> Result<(), String> {
    let mut socket =
        dial_upstream(upstream, hub, pin, crate::wire::CONTROL_PATH, crate::wire::MAX_CONTROL_FRAME).await?.socket;
    let hello = Up::Hello {
        protocol: crate::wire::GREETING_PROTOCOL,
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
    let Some(bytes) = next(socket).await? else { return Ok(None) };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| format!("a frame from the hub: {error}"))
}

/// A frame as a short session waiting for one answer reads it, such as a
/// withdrawal while the machine's node is down: the hub sends it whatever
/// it sends the machine, calls of other machines among them, in forms a
/// newer hub or caller may have that this build cannot read.
pub enum Heard {
    Frame(Box<Down>),
    /// A frame this build cannot read, which is not the answer it waits for.
    Unread,
}

/// As `recv`, a frame this build cannot read being `Heard::Unread`.
pub async fn hear<S>(socket: &mut S) -> Result<Option<Heard>, String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let Some(bytes) = next(socket).await? else { return Ok(None) };
    Ok(Some(serde_json::from_slice(&bytes).map_or(Heard::Unread, |frame| Heard::Frame(Box::new(frame)))))
}

/// The next binary message; `None` when the hub closed the connection.
async fn next<S>(socket: &mut S) -> Result<Option<tokio_tungstenite::tungstenite::Bytes>, String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match socket.next().await {
            None | Some(Ok(WsMessage::Close(_))) => return Ok(None),
            Some(Ok(WsMessage::Binary(bytes))) => return Ok(Some(bytes)),
            Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
            Some(Ok(WsMessage::Text(_))) => {
                return Err("the hub sent text, not a frame".to_string());
            }
            Some(Err(error)) => return Err(describe(&error)),
        }
    }
}
