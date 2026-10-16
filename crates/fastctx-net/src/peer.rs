//! Direct links: what two machines say to each other without the hub
//! (`world-network.md`, 直连).
//!
//! A machine the network's administrator allowed direct connections to
//! listens on a port. Another machine reaches it over TLS, as it reaches
//! the hub, on one of three paths: a direct link, which both keep open and
//! call each other over; a data connection that goes with a call made over
//! a direct link; and a forward, which carries the hub's own TLS from a
//! machine whose line to the hub is down, unread, to the hub. The hub also
//! comes once, when the direct connections are allowed, to check from
//! outside that it reaches the machine at the addresses chosen.
//!
//! The listening machine's certificate is made when it starts listening and
//! proves nothing; each side proves who it is by signing the TLS exporter
//! with its machine key, and each checks the other's key against the
//! roster the hub gave it (`crate::directory::View`).

use std::net::SocketAddr;
use std::sync::Arc;

use fastctx_types::MachineName;
use futures_util::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::envelope::Envelope;
use crate::ids::HubId;
use crate::sign::{PublicKey, Signature};
use crate::wire::{End, SILENCE};

/// The path of a direct link between two machines.
pub const LINK_PATH: &str = "/fastctx/world/4/peer";
/// The path of a data connection that goes with a call over a direct link.
pub const DATA_PATH: &str = "/fastctx/world/4/peer/data";
/// The path of a connection forwarded to the hub, followed by `/<hub id>`.
pub const FORWARD_PATH: &str = "/fastctx/world/4/forward";

/// Raised whenever a change would make two machines misread each other.
pub const PROTOCOL: u32 = 1;

/// The port a machine takes direct connections at when none is chosen: the
/// one after the port hubs are commonly reached at.
pub const DEFAULT_PORT: u16 = 7444;

/// The largest message on a forward: the hub's TLS records in either
/// direction, which are never larger than a data connection's messages
/// with their framing.
pub const MAX_FORWARD_MESSAGE: usize = crate::wire::MAX_DATA_MESSAGE + crate::wire::MAX_CONTROL_FRAME;

/// A frame of a direct link, or the first ones of a data connection or a
/// probe. Every frame is one JSON value in a binary WebSocket message.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "f")]
pub enum Frame {
    /// The first frame, from the machine that connected.
    Hello {
        protocol: u32,
        version: String,
        hub: HubId,
        intent: Intent,
    },
    /// The listening machine took the hello: it is `name`, and `proof` is
    /// its key's signature of the TLS exporter.
    Welcome {
        name: MachineName,
        proof: Signature,
        /// It reads `Cancel` on a link. Earlier releases do not say so, and
        /// end the link at a frame they cannot read.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cancels: bool,
    },
    /// The connection is refused or ended, and why.
    Refused {
        reason: String,
    },
    /// A heartbeat; `took`, how many bytes of this connection the sender
    /// took from the receiver (`crate::pulse`), which an earlier release
    /// does not say.
    Ping {
        n: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        took: Option<u64>,
    },
    Pong {
        n: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        took: Option<u64>,
    },
    /// A call for the machine on the other end, numbered `id` by its caller.
    Call {
        id: u64,
        message: Envelope,
    },
    /// The answer to the call the receiver's caller numbered `id`.
    Answer {
        id: u64,
        message: Envelope,
    },
    /// The machine answering the call `id` stopped: the right it was made
    /// under was taken back. Whether it did what was asked is not known.
    Cut {
        id: u64,
        reason: String,
    },
    /// The caller of the call it numbered `id` gave it up: one that only
    /// reads stops (`fastctx_types::contract`). Sent only to a machine that
    /// says it reads this.
    Cancel {
        id: u64,
    },
}

/// What a connection to a listening machine is for.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Intent {
    /// A direct link of the machine whose key `key` is; `proof` is its
    /// signature of the TLS exporter; `cancels`, whether it reads `Cancel`.
    Link {
        key: PublicKey,
        proof: Signature,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cancels: bool,
    },
    /// The connecting machine's end of the data connection `stream`.
    Data { key: PublicKey, proof: Signature, stream: u64, end: End },
    /// The hub checking it reaches this machine here.
    Probe,
}

/// Encodes a frame for the wire.
pub fn encode(frame: &Frame) -> Vec<u8> {
    serde_json::to_vec(frame).expect("frames always serialize")
}

/// Sends one frame.
pub async fn send<S>(socket: &mut S, frame: &Frame) -> Result<(), String>
where
    S: futures_util::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    socket.send(WsMessage::Binary(encode(frame).into())).await.map_err(|error| crate::dial::describe(&error))
}

/// Reads the next frame; `None` when the other end closed the connection.
pub async fn recv<S>(socket: &mut S) -> Result<Option<Frame>, String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match socket.next().await {
            None | Some(Ok(WsMessage::Close(_))) => return Ok(None),
            Some(Ok(WsMessage::Binary(bytes))) => {
                return serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|error| format!("an unreadable frame: {error}"));
            }
            Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
            Some(Ok(WsMessage::Text(_))) => return Err("text instead of a frame".to_string()),
            Some(Err(error)) => return Err(crate::dial::describe(&error)),
        }
    }
}

/// The next frame within the silence the World allows a peer.
pub async fn next(socket: &mut crate::transport::Socket) -> Result<Frame, String> {
    match tokio::time::timeout(SILENCE, recv(socket)).await {
        Ok(Ok(Some(frame))) => Ok(frame),
        Ok(Ok(None)) => Err("the other end closed the connection".to_string()),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(format!("nothing was heard for {} s", SILENCE.as_secs())),
    }
}

/// The TLS a listening machine shows: a certificate made now, which
/// proves nothing, since the machine proves who it is with its key.
pub fn listener_tls() -> Result<Arc<rustls::ServerConfig>, String> {
    crate::tls::HubCertificate::generate_for("fastctx-node")?.server_config()
}

/// How the hub's check of a machine's address went wrong, in terms of which
/// layer stopped it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unreached {
    /// No TCP answer came: packets to the port were dropped on the way, by
    /// a firewall on the machine, a cloud provider's security group, or a
    /// router.
    Silent,
    /// The connection was refused: something on the path, or the computer
    /// at that address, said nothing listens there.
    Refused(String),
    /// Something answered, but not this machine as itself.
    Other(String),
}

impl std::fmt::Display for Unreached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unreached::Silent => write!(f, "nothing answered within {} s", SILENCE.as_secs()),
            Unreached::Refused(error) => write!(f, "the connection was refused ({error})"),
            Unreached::Other(error) => write!(f, "something answered, but not the machine: {error}"),
        }
    }
}

/// Checks, as the hub does from where it is, that the machine `name` whose
/// key is `key` is reached at `address` for the hub `hub`.
pub async fn probe(address: SocketAddr, hub: &HubId, name: &MachineName, key: &PublicKey) -> Result<(), Unreached> {
    let tcp = match tokio::time::timeout(SILENCE, tokio::net::TcpStream::connect(address)).await {
        Err(_) => return Err(Unreached::Silent),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::TimedOut => return Err(Unreached::Silent),
        Ok(Err(error)) => return Err(Unreached::Refused(error.to_string())),
        Ok(Ok(tcp)) => tcp,
    };
    crate::dial::configure(&tcp);
    let checked = async {
        let tls = tokio_rustls::TlsConnector::from(crate::tls::client_config(None))
            .connect(
                rustls::pki_types::ServerName::IpAddress(address.ip().into()),
                crate::transport::Transport::tcp(tcp),
            )
            .await
            .map_err(|error| format!("TLS: {error}"))?;
        let mut binding = [0_u8; 32];
        tls.get_ref()
            .1
            .export_keying_material(&mut binding, crate::wire::EXPORTER_LABEL, None)
            .map_err(|error| format!("TLS exporter: {error}"))?;
        let (mut socket, _) = tokio_tungstenite::client_async_with_config(
            format!("ws://{address}{LINK_PATH}"),
            tokio_rustls::TlsStream::Client(tls),
            Some(crate::dial::config(crate::wire::MAX_CONTROL_FRAME)),
        )
        .await
        .map_err(|error| format!("WebSocket: {error}"))?;
        let hello = Frame::Hello {
            protocol: PROTOCOL,
            version: env!("CARGO_PKG_VERSION").to_string(),
            hub: hub.clone(),
            intent: Intent::Probe,
        };
        send(&mut socket, &hello).await?;
        let answer = next(&mut socket).await;
        let _ = socket.close(None).await;
        match answer? {
            Frame::Welcome { name: said, proof, .. } => {
                if said != *name {
                    return Err(format!("{said} answered, not {name}"));
                }
                key.verify(crate::sign::Domain::Peer, &binding, &proof)
                    .map_err(|_| format!("what answered does not hold {name}'s key"))
            }
            Frame::Refused { reason } => Err(reason),
            _ => Err("it answered something other than a welcome".to_string()),
        }
    };
    match tokio::time::timeout(SILENCE, checked).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(Unreached::Other(error)),
        Err(_) => Err(Unreached::Other(format!("no welcome within {} s", SILENCE.as_secs()))),
    }
}

/// Whether `address` is one only machines on the same network reach: the
/// hub, from elsewhere, cannot check it.
pub fn local_only(address: &std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || {
                // Carrier-grade NAT, 100.64.0.0/10.
                let [a, b, ..] = v4.octets();
                a == 100 && (64..128).contains(&b)
            }
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}
