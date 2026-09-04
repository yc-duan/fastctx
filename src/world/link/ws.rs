//! Dialing the hub: TCP on a chosen path, TLS 1.3, the WebSocket upgrade, and the channel
//! binding exported from the TLS connection.

use super::netpath::{Interface, NetworkView, pin_socket};
use super::proxy::ProxyConfig;
use super::tls::{Learned, Verify};
use crate::world::wire::{BINDING_LEN, EXPORTER_LABEL, Frame};
use crate::world::{NetworkMode, WS_PATH};
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// One connection attempt, TCP through upgrade.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A parsed hub address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Endpoint {
    pub(crate) host: String,
    pub(crate) port: u16,
}

impl Endpoint {
    /// Parses `host`, `host:port`, `[v6]:port`, or `https://host[:port]`.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let text = text
            .strip_prefix("https://")
            .or_else(|| text.strip_prefix("wss://"))
            .unwrap_or(text)
            .trim_end_matches('/');
        if text.is_empty() {
            return Err("The hub address is empty.".to_string());
        }
        if let Some(rest) = text.strip_prefix('[') {
            let (host, port) = rest
                .split_once(']')
                .ok_or_else(|| format!("Invalid hub address \"{text}\"."))?;
            let port = port
                .strip_prefix(':')
                .map(|port| {
                    port.parse::<u16>()
                        .map_err(|_| format!("Invalid port in \"{text}\"."))
                })
                .transpose()?
                .unwrap_or(443);
            return Ok(Self {
                host: host.to_string(),
                port,
            });
        }
        if let Ok(address) = text.parse::<std::net::Ipv6Addr>() {
            return Ok(Self {
                host: address.to_string(),
                port: 443,
            });
        }
        match text.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() => Ok(Self {
                host: host.to_string(),
                port: port
                    .parse::<u16>()
                    .map_err(|_| format!("Invalid port in \"{text}\"."))?,
            }),
            _ => Ok(Self {
                host: text.to_string(),
                port: 443,
            }),
        }
    }

    pub(crate) fn ip_literal(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }

    pub(crate) fn server_name(&self) -> Result<ServerName<'static>, String> {
        ServerName::try_from(self.host.clone())
            .map_err(|_| format!("\"{}\" is not a valid TLS server name.", self.host))
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.host.contains(':') {
            write!(formatter, "[{}]:{}", self.host, self.port)
        } else {
            write!(formatter, "{}:{}", self.host, self.port)
        }
    }
}

/// How an established link left the machine.
#[derive(Clone, Debug)]
pub(crate) enum Path {
    Direct {
        interface: String,
        local: SocketAddr,
        remote: SocketAddr,
        resolvers: Vec<IpAddr>,
        tunnels: Vec<String>,
    },
    System {
        proxy: Option<String>,
        local: Option<SocketAddr>,
        remote: Option<SocketAddr>,
    },
}

impl Path {
    pub(crate) fn mode(&self) -> NetworkMode {
        match self {
            Self::Direct { .. } => NetworkMode::Direct,
            Self::System { .. } => NetworkMode::System,
        }
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Direct {
                interface,
                local,
                resolvers,
                ..
            } => {
                let dns = if resolvers.is_empty() {
                    String::new()
                } else {
                    format!(
                        ", dns {}",
                        resolvers
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                };
                format!("direct via \"{interface}\" ({}){dns}", local.ip())
            }
            Self::System {
                proxy: Some(proxy), ..
            } => format!("system via HTTPS_PROXY {proxy}"),
            Self::System {
                local: Some(local), ..
            } => format!("system ({})", local.ip()),
            Self::System { .. } => "system".to_string(),
        }
    }
}

pub(crate) type Socket = WebSocketStream<TlsStream<TcpStream>>;

/// A dialed hub connection before the application handshake.
pub(crate) struct Dialed {
    pub(crate) socket: Socket,
    pub(crate) binding: Vec<u8>,
    pub(crate) path: Path,
    pub(crate) learned: Option<Learned>,
    pub(crate) endpoint: Endpoint,
}

impl Dialed {
    pub(crate) async fn send(&mut self, frame: &Frame) -> Result<(), String> {
        let bytes = frame.encode()?;
        self.socket
            .send(Message::Binary(bytes.into()))
            .await
            .map_err(|error| format!("cannot write to the hub: {error}"))
    }

    /// Reads the next frame; `Ok(None)` when the hub closed the connection.
    pub(crate) async fn recv(&mut self) -> Result<Option<Frame>, String> {
        loop {
            match self.socket.next().await {
                None => return Ok(None),
                Some(Err(error)) => return Err(format!("cannot read from the hub: {error}")),
                Some(Ok(Message::Binary(bytes))) => return Frame::decode(&bytes).map(Some),
                Some(Ok(Message::Text(text))) => return Frame::decode(text.as_bytes()).map(Some),
                Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            }
        }
    }

    pub(crate) async fn close(&mut self) {
        let _ = self.socket.close(None).await;
    }
}

fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(crate::world::wire::MAX_FRAME_BYTES))
        .max_frame_size(Some(crate::world::wire::MAX_FRAME_BYTES))
}

fn configure_tcp(stream: &TcpStream) {
    let socket = socket2::SockRef::from(stream);
    let _ =
        socket.set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(Duration::from_secs(30)));
    let _ = socket.set_tcp_nodelay(true);
}

/// Dials through a pinned physical interface with the interface's own resolvers.
pub(crate) async fn dial_direct(
    endpoint: &Endpoint,
    verify: &Verify,
    view: &NetworkView,
    interface: &Interface,
) -> Result<Dialed, String> {
    let addresses = super::dns::resolve(&endpoint.host, interface).await?;
    let mut last_error = String::new();
    for address in addresses {
        let ipv6 = address.is_ipv6();
        if interface.local_address(ipv6).is_none() {
            continue;
        }
        let socket = if ipv6 {
            tokio::net::TcpSocket::new_v6()
        } else {
            tokio::net::TcpSocket::new_v4()
        }
        .map_err(|error| format!("cannot create a socket: {error}"))?;
        pin_socket(&socket2::SockRef::from(&socket), interface, ipv6)?;
        let remote = SocketAddr::new(address, endpoint.port);
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, socket.connect(remote)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                last_error = format!(
                    "cannot connect to {remote} via \"{}\": {error}",
                    interface.name
                );
                continue;
            }
            Err(_) => {
                last_error = format!(
                    "connecting to {remote} via \"{}\" timed out",
                    interface.name
                );
                continue;
            }
        };
        configure_tcp(&stream);
        let local = stream.local_addr().map_err(|error| error.to_string())?;
        let path = Path::Direct {
            interface: interface.name.clone(),
            local,
            remote,
            resolvers: interface.resolvers(),
            tunnels: view.tunnels(),
        };
        return upgrade(endpoint, verify, stream, path).await;
    }
    if last_error.is_empty() {
        last_error = format!(
            "\"{}\" has no address family in common with the hub's addresses.",
            interface.name
        );
    }
    Err(last_error)
}

/// Dials with the operating system's routing, resolver, and proxy.
pub(crate) async fn dial_system(
    endpoint: &Endpoint,
    verify: &Verify,
    proxy: Option<&ProxyConfig>,
) -> Result<Dialed, String> {
    let (stream, proxy_text) = match proxy {
        Some(proxy) => (
            tokio::time::timeout(
                CONNECT_TIMEOUT,
                super::proxy::connect_through(proxy, &endpoint.host, endpoint.port),
            )
            .await
            .map_err(|_| {
                format!(
                    "connecting through the proxy {} timed out",
                    proxy.describe()
                )
            })??,
            Some(proxy.describe()),
        ),
        None => (
            tokio::time::timeout(
                CONNECT_TIMEOUT,
                TcpStream::connect((endpoint.host.as_str(), endpoint.port)),
            )
            .await
            .map_err(|_| format!("connecting to {endpoint} timed out"))?
            .map_err(|error| format!("cannot connect to {endpoint}: {error}"))?,
            None,
        ),
    };
    configure_tcp(&stream);
    let path = Path::System {
        proxy: proxy_text,
        local: stream.local_addr().ok(),
        remote: stream.peer_addr().ok(),
    };
    upgrade(endpoint, verify, stream, path).await
}

async fn upgrade(
    endpoint: &Endpoint,
    verify: &Verify,
    stream: TcpStream,
    path: Path,
) -> Result<Dialed, String> {
    let learned_slot = match verify {
        Verify::Learn(slot) => Some(Arc::clone(slot)),
        _ => None,
    };
    let config = super::tls::client_config(verify)?;
    let connector = TlsConnector::from(config);
    let tls = tokio::time::timeout(
        CONNECT_TIMEOUT,
        connector.connect(endpoint.server_name()?, stream),
    )
    .await
    .map_err(|_| format!("the TLS handshake with {endpoint} timed out"))?
    .map_err(|error| format!("the TLS handshake with {endpoint} failed: {error}"))?;
    let binding = tls
        .get_ref()
        .1
        .export_keying_material(vec![0_u8; BINDING_LEN], EXPORTER_LABEL, None)
        .map_err(|error| format!("cannot export the TLS channel binding: {error}"))?;
    let request = format!("wss://{endpoint}{WS_PATH}")
        .into_client_request()
        .map_err(|error| format!("cannot build the upgrade request: {error}"))?;
    let (socket, _response) = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::client_async_with_config(request, tls, Some(websocket_config())),
    )
    .await
    .map_err(|_| format!("the WebSocket upgrade with {endpoint} timed out"))?
    .map_err(|error| {
        format!("the hub at {endpoint} did not accept the WebSocket upgrade: {error}")
    })?;
    let learned = learned_slot.and_then(|slot| slot.lock().clone());
    Ok(Dialed {
        socket,
        binding,
        path,
        learned,
        endpoint: endpoint.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::Endpoint;

    #[test]
    fn hub_addresses_parse_in_every_written_form() {
        assert_eq!(
            Endpoint::parse("hub.example").unwrap(),
            Endpoint {
                host: "hub.example".into(),
                port: 443
            }
        );
        assert_eq!(
            Endpoint::parse("https://hub.example:7443/").unwrap(),
            Endpoint {
                host: "hub.example".into(),
                port: 7443
            }
        );
        assert_eq!(
            Endpoint::parse("121.40.82.28:7443").unwrap(),
            Endpoint {
                host: "121.40.82.28".into(),
                port: 7443
            }
        );
        assert_eq!(
            Endpoint::parse("[2001:db8::1]:8443").unwrap(),
            Endpoint {
                host: "2001:db8::1".into(),
                port: 8443
            }
        );
        assert_eq!(Endpoint::parse("2001:db8::1").unwrap().port, 443);
        assert!(Endpoint::parse("hub.example:notaport").is_err());
        assert_eq!(
            Endpoint::parse("[2001:db8::1]:8443").unwrap().to_string(),
            "[2001:db8::1]:8443"
        );
    }
}
