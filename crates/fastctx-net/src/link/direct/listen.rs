//! Taking direct connections: the listener of a machine that allows them.
//!
//! Every connection is TLS on a certificate made when listening began, then
//! a WebSocket whose path says what it is for (`crate::peer`). Whoever
//! connects for a link or a data connection proves its key, and is taken
//! only when that key is a member's in the roster this machine last had;
//! the hub's check is answered with this machine's own proof; a forward is
//! carried to the hub byte for byte, for any machine, since the hub itself
//! checks who it is. A connection that does not get that far is logged,
//! with where it came from and why.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, Weak};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

use super::super::Inner;
use super::Listening;
use crate::peer::{self, Frame, Intent};
use crate::sign::Domain;
use crate::transport::{Socket, Transport};
use crate::wire::{EXPORTER_LABEL, SILENCE};

/// Listens at the `wanted` ports, on every interface, and no longer at
/// others; a port listened at already is left as it is. Whether it began
/// listening anywhere.
pub(super) fn follow(inner: &Arc<Inner>, listening: &mut Listening, wanted: &BTreeSet<(bool, u16)>) -> bool {
    listening.ports.retain(|port, task| {
        let keep = wanted.contains(port);
        if !keep && let Ok(task) = task {
            task.abort();
            inner.node.log(format!("direct connections: no longer listening at port {}", port.1));
        }
        keep
    });
    let mut began = false;
    for (v6, port) in wanted {
        if listening.ports.contains_key(&(*v6, *port)) {
            continue;
        }
        began = true;
        let address = SocketAddr::new(
            if *v6 { IpAddr::V6(Ipv6Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::UNSPECIFIED) },
            *port,
        );
        let tls = match &listening.tls {
            Some(tls) => tls.clone(),
            None => match peer::listener_tls() {
                Ok(config) => listening.tls.insert(TlsAcceptor::from(config)).clone(),
                Err(error) => {
                    let failed = format!("making the certificate it shows: {error}");
                    inner.node.log(format!("direct connections: {failed}"));
                    listening.ports.insert((*v6, *port), Err(failed));
                    continue;
                }
            },
        };
        let task = match crate::listen::bind(address) {
            Ok(listener) => {
                inner.node.log(format!("direct connections: listening at {address}"));
                Ok(tokio::spawn(accept(Arc::downgrade(inner), listener, tls)))
            }
            Err(error) => {
                let failed = format!("listening at {address}: {error}");
                inner.node.log(format!("direct connections: {failed}"));
                Err(failed)
            }
        };
        listening.ports.insert((*v6, *port), task);
    }
    began
}

async fn accept(inner: Weak<Inner>, listener: TcpListener, tls: TlsAcceptor) {
    loop {
        match listener.accept().await {
            Ok((tcp, from)) => {
                crate::dial::configure(&tcp);
                tokio::spawn(connected(inner.clone(), tcp, from, tls.clone()));
            }
            // Out of descriptors, for one: the next accept may succeed.
            Err(error) => {
                if let Some(inner) = inner.upgrade() {
                    inner.node.log(format!("direct connections: accepting: {error}"));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

async fn connected(inner: Weak<Inner>, tcp: tokio::net::TcpStream, from: SocketAddr, tls: TlsAcceptor) {
    let Some(inner) = inner.upgrade() else { return };
    let log = |what: String| inner.node.log(format!("direct connections: {from}: {what}"));
    let deadline = Instant::now() + SILENCE;
    let stream = match tokio::time::timeout_at(deadline, tls.accept(Transport::Tcp(tcp))).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => return log(format!("the TLS handshake failed: {error}")),
        Err(_) => return log(format!("no TLS handshake within {} s", SILENCE.as_secs())),
    };
    let mut binding = [0_u8; 32];
    if let Err(error) = stream.get_ref().1.export_keying_material(&mut binding, EXPORTER_LABEL, None) {
        return log(format!("the TLS exporter failed: {error}"));
    }
    let path = Arc::new(Mutex::new(String::new()));
    let seen = path.clone();
    let callback = move |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        *seen.lock().expect("never poisoned") = request.uri().path().to_string();
        Ok(response)
    };
    let config = crate::dial::config(peer::MAX_FORWARD_MESSAGE);
    let opened = tokio_tungstenite::accept_hdr_async_with_config(
        tokio_rustls::TlsStream::Server(stream),
        callback,
        Some(config),
    );
    let mut socket = match tokio::time::timeout_at(deadline, opened).await {
        Ok(Ok(socket)) => socket,
        Ok(Err(error)) => return log(format!("the WebSocket did not open: {error}")),
        Err(_) => return log(format!("no WebSocket within {} s", SILENCE.as_secs())),
    };
    let path = path.lock().expect("never poisoned").clone();
    let forward = format!("{}/{}", peer::FORWARD_PATH, inner.hub);
    if path == forward {
        return forward_to_hub(inner, socket, from).await;
    }
    if path != peer::LINK_PATH && path != peer::DATA_PATH {
        log(format!("refused a connection on {path}"));
        let _ = peer::send(&mut socket, &Frame::Refused { reason: format!("nothing is taken on {path}") }).await;
        return;
    }
    let hello = match tokio::time::timeout_at(deadline, peer::recv(&mut socket)).await {
        Ok(Ok(Some(hello))) => hello,
        Ok(Ok(None)) => return log("it closed the connection before saying hello".to_string()),
        Ok(Err(error)) => return log(format!("its hello: {error}")),
        Err(_) => return log(format!("no hello within {} s", SILENCE.as_secs())),
    };
    let refuse = async |socket: &mut Socket, reason: String| {
        log(format!("refused: {reason}"));
        let _ = peer::send(socket, &Frame::Refused { reason }).await;
        let _ = socket.close(None).await;
    };
    let Frame::Hello { protocol, hub, intent, .. } = hello else {
        return refuse(&mut socket, "the first frame must be a hello".to_string()).await;
    };
    if protocol != peer::PROTOCOL {
        let reason = format!("it speaks direct protocol {protocol}, this machine {}", peer::PROTOCOL);
        return refuse(&mut socket, reason).await;
    }
    if hub != inner.hub {
        return refuse(&mut socket, format!("this machine takes direct connections for another hub than {hub}")).await;
    }
    let me = inner.me();
    let welcome = Frame::Welcome { name: me.clone(), proof: inner.key.sign(Domain::Peer, &binding) };
    // Who connects, by the key it proves it holds, from the roster this
    // machine last had: also while it cannot reach the hub.
    let member = |key: &crate::sign::PublicKey,
                  proof: &crate::sign::Signature|
     -> Result<fastctx_types::MachineName, String> {
        key.verify(Domain::Peer, &binding, proof).map_err(|_| "the proof of its key does not verify".to_string())?;
        let view = inner.view();
        let member = view.by_key(key).ok_or_else(|| "its key is no member's in this machine's roster".to_string())?;
        if member.name == me {
            return Err("it holds this machine's own key".to_string());
        }
        Ok(member.name.clone())
    };
    match (path.as_str(), intent) {
        (peer::LINK_PATH, Intent::Probe) => {
            let _ = peer::send(&mut socket, &welcome).await;
            let _ = socket.close(None).await;
            log("the hub, or someone, checked that this machine is reached here".to_string());
        }
        (peer::LINK_PATH, Intent::Link { key, proof }) => match member(&key, &proof) {
            Ok(name) => {
                if peer::send(&mut socket, &welcome).await.is_ok() {
                    super::peer::taken(inner, name, key, socket, from).await;
                }
            }
            Err(reason) => refuse(&mut socket, reason).await,
        },
        (peer::DATA_PATH, Intent::Data { key, proof, stream, end }) => match member(&key, &proof) {
            Ok(name) => {
                if peer::send(&mut socket, &welcome).await.is_ok() {
                    inner.arrived(name, stream, end, socket);
                }
            }
            Err(reason) => refuse(&mut socket, reason).await,
        },
        (path, _) => refuse(&mut socket, format!("that is not said on {path}")).await,
    }
}

/// Carries a connection to the hub, byte for byte both ways, until either
/// side ends it. The hub's TLS inside is not this machine's to read.
async fn forward_to_hub(inner: Arc<Inner>, socket: Socket, from: SocketAddr) {
    let addresses = inner.hub_addresses();
    let tcp = match crate::dial::connect_tcp(&addresses).await {
        Ok(tcp) => tcp,
        Err(error) => {
            inner.node.log(format!("direct connections: {from}: forwarding to the hub failed: {error}"));
            return;
        }
    };
    drop(inner);
    let (mut sink, mut stream) = socket.split();
    let (mut read, mut write) = tcp.into_split();
    let up = async {
        while let Some(Ok(message)) = stream.next().await {
            match message {
                WsMessage::Binary(bytes) => {
                    if write.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                WsMessage::Close(_) => break,
                _ => {}
            }
        }
        let _ = write.shutdown().await;
    };
    let down = async {
        // How much is read from the hub at once; any size carries the bytes.
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            match read.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if sink.send(WsMessage::Binary(bytes::Bytes::copy_from_slice(&buffer[..read]))).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = sink.close().await;
    };
    // A close does not pass through as half a close: when either side is
    // done, the forward is.
    tokio::select! {
        () = up => {}
        () = down => {}
    }
}
