//! One member connection at the hub: the handshake, the replacement rule, heartbeats, acks,
//! and the frame loop that hands application envelopes to the router.

use super::store::{MemberRow, OutboxRow};
use super::{Connected, Hub, log};
use crate::world::PROTOCOL_VERSION;
use crate::world::crypto::{self, b64_array, b64_decode, b64_encode};
use crate::world::identity::verify;
use crate::world::invite::Invite;
use crate::world::wire::{
    self, Auth, BindingMode, Challenge, Enrolled, Frame, Hello, Intent, ProtocolMismatch, Rejected,
    Welcome,
};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

/// The handshake must finish within this window.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Three missed heartbeats.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);

/// Who is on the other end of an authenticated connection.
#[derive(Clone, Debug)]
pub(crate) struct Peer {
    pub(crate) name: String,
    pub(crate) node_pub: crypto::Key32,
    pub(crate) generation: u64,
    /// fastctx version the member announced.
    pub(crate) version: String,
    pub(crate) protocol: u32,
}

pub(crate) struct Link<S> {
    socket: WebSocketStream<S>,
    peer: SocketAddr,
}

impl<S> Link<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    async fn send(&mut self, frame: &Frame) -> Result<(), String> {
        let bytes = frame.encode()?;
        self.socket
            .send(Message::Binary(bytes.into()))
            .await
            .map_err(|error| format!("cannot write to {}: {error}", self.peer))
    }

    /// Reads the next frame; `Ok(None)` when the peer closed the connection.
    async fn recv(&mut self) -> Result<Option<Frame>, String> {
        loop {
            match self.socket.next().await {
                None => return Ok(None),
                Some(Err(error)) => return Err(format!("cannot read from {}: {error}", self.peer)),
                Some(Ok(Message::Binary(bytes))) => return Frame::decode(&bytes).map(Some),
                Some(Ok(Message::Text(text))) => return Frame::decode(text.as_bytes()).map(Some),
                Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            }
        }
    }

    async fn close(&mut self) {
        let _ = self.socket.close(None).await;
    }
}

/// Runs a connection from upgrade to close.
pub(crate) async fn serve<S>(
    hub: Arc<Hub>,
    socket: WebSocketStream<S>,
    binding: Vec<u8>,
    peer: SocketAddr,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut link = Link { socket, peer };
    let (peer_identity, auth) =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&hub, &mut link, &binding)).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(HandshakeFailure::Rejected(rejected))) => {
                log(format!(
                    "{peer}: rejected ({}): {}",
                    rejected.code, rejected.message
                ));
                let _ = link.send(&Frame::Rejected(rejected)).await;
                link.close().await;
                return;
            }
            Ok(Err(HandshakeFailure::Transport(error))) => {
                log(format!("{peer}: handshake failed: {error}"));
                link.close().await;
                return;
            }
            Err(_) => {
                log(format!("{peer}: handshake timed out"));
                link.close().await;
                return;
            }
        };
    run_session(hub, link, peer_identity, auth).await;
}

enum HandshakeFailure {
    Rejected(Rejected),
    Transport(String),
}

impl From<String> for HandshakeFailure {
    fn from(error: String) -> Self {
        Self::Transport(error)
    }
}

fn reject(code: &str, message: impl Into<String>) -> HandshakeFailure {
    HandshakeFailure::Rejected(Rejected {
        code: code.to_string(),
        message: message.into(),
        protocol: None,
    })
}

async fn handshake<S>(
    hub: &Arc<Hub>,
    link: &mut Link<S>,
    binding: &[u8],
) -> Result<(Peer, Auth), HandshakeFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let hello = read_hello(link).await?;
    if hello.protocol < PROTOCOL_VERSION.saturating_sub(1) || hello.min_protocol > PROTOCOL_VERSION
    {
        return Err(HandshakeFailure::Rejected(Rejected {
            code: "protocol_mismatch".to_string(),
            message: format!(
                "This hub speaks World protocol {PROTOCOL_VERSION} and accepts {} or newer; the member offered {} (minimum {}). Upgrade the older side.",
                PROTOCOL_VERSION.saturating_sub(1),
                hello.protocol,
                hello.min_protocol
            ),
            protocol: Some(ProtocolMismatch {
                ours: PROTOCOL_VERSION,
                theirs: hello.protocol,
            }),
        }));
    }
    let node_nonce = b64_decode(&hello.nonce).map_err(|error| reject("invalid_hello", error))?;
    let node_pub = b64_array::<32>(&hello.node_pub, "member public key")
        .map_err(|error| reject("invalid_hello", error))?;
    let wrap_pub = b64_array::<32>(&hello.wrap_pub, "member wrap key")
        .map_err(|error| reject("invalid_hello", error))?;
    let hub_nonce = crypto::random_bytes::<32>()?;
    let hub_pub = hub.identity.public_key();
    let transcript = wire::hub_transcript(&node_nonce, &hub_nonce, &hub_pub, &node_pub, binding);
    let challenge = Challenge {
        protocol: PROTOCOL_VERSION,
        nonce: b64_encode(&hub_nonce),
        hub_pub: b64_encode(&hub_pub),
        world_id: hub.world_id.clone(),
        binding: hub.binding_mode,
        sig: b64_encode(&hub.identity.sign(wire::HUB_HANDSHAKE_DOMAIN, &transcript)),
    };
    link.send(&Frame::Challenge(challenge)).await?;

    let auth = match link.recv().await? {
        Some(Frame::Auth(auth)) => auth,
        Some(_) => {
            return Err(reject(
                "invalid_handshake",
                "Expected auth after challenge.",
            ));
        }
        None => {
            return Err(HandshakeFailure::Transport(
                "the member closed during the handshake".to_string(),
            ));
        }
    };
    let node_transcript =
        wire::node_transcript(&hub_nonce, &node_nonce, &node_pub, &hub_pub, binding);
    let signature = b64_decode(&auth.sig).map_err(|error| reject("invalid_handshake", error))?;
    verify(
        &node_pub,
        wire::NODE_HANDSHAKE_DOMAIN,
        &node_transcript,
        &signature,
    )
    .map_err(|_| {
        reject(
            "invalid_handshake",
            "The auth signature does not verify for the presented public key.",
        )
    })?;

    let node_pub_b64 = hello.node_pub.clone();
    let name = match hello.intent {
        Intent::Auth => {
            let Some(row) = hub.store.member_by_key(&node_pub_b64)? else {
                return Err(reject(
                    "not_enrolled",
                    "This machine's key is not enrolled in this World. Ask a member for an invite.",
                ));
            };
            if row.is_revoked() {
                return Err(reject(
                    "revoked",
                    format!("The member \"{}\" was revoked from this World.", row.name),
                ));
            }
            row.name
        }
        Intent::Enroll => enroll(hub, &hello, &auth, &node_pub_b64, &wrap_pub)?,
        Intent::Bootstrap => bootstrap(hub, &hello, &auth, &node_pub_b64, &wrap_pub)?,
    };
    let generation = hub.next_generation(&name);
    Ok((
        Peer {
            name,
            node_pub,
            generation,
            version: hello.version.clone(),
            protocol: hello.protocol,
        },
        auth,
    ))
}

async fn read_hello<S>(link: &mut Link<S>) -> Result<Hello, HandshakeFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match link.recv().await? {
        Some(Frame::Hello(hello)) => Ok(hello),
        Some(_) => Err(reject("invalid_handshake", "Expected hello first.")),
        None => Err(HandshakeFailure::Transport(
            "the peer closed before saying hello".to_string(),
        )),
    }
}

fn enroll(
    hub: &Hub,
    hello: &Hello,
    auth: &Auth,
    node_pub_b64: &str,
    wrap_pub: &crypto::Key32,
) -> Result<String, HandshakeFailure> {
    let Some(enrollment) = &auth.enrollment else {
        return Err(reject("invite_invalid", "Enrollment needs an invite."));
    };
    // Every failure below reads the same to the peer so an invite cannot be enumerated.
    let invalid = || {
        reject(
            "invite_invalid",
            "That invite is not valid: it may have expired, been used already, or never existed. Ask a member for a new one.",
        )
    };
    let Some(invite) = hub.store.invite(&enrollment.code_id)? else {
        return Err(invalid());
    };
    if crate::world::parse_rfc3339(&invite.exp)
        .is_ok_and(|exp| exp <= time::OffsetDateTime::now_utc())
    {
        let _ = hub.store.remove_invite(&enrollment.code_id);
        return Err(invalid());
    }
    let token = b64_decode(&enrollment.admission_token).map_err(|_| invalid())?;
    if Invite::admission_of_token(&token) != invite.admission {
        return Err(invalid());
    }
    crate::world::validate_node_name(&enrollment.name)
        .map_err(|error| reject("invalid_name", error))?;
    if let Some(existing) = hub.store.member(&enrollment.name)?
        && existing.node_pub != node_pub_b64
    {
        return Err(reject(
            "node_name_taken",
            format!(
                "The name \"{}\" already belongs to another member.",
                enrollment.name
            ),
        ));
    }
    if let Some(existing) = hub.store.member_by_key(node_pub_b64)?
        && existing.name != enrollment.name
    {
        return Err(reject(
            "already_enrolled",
            format!(
                "This machine's key is already enrolled as \"{}\". Run 'fastctx node unenroll' there first.",
                existing.name
            ),
        ));
    }
    let now = crate::world::now_rfc3339();
    hub.store.put_member(&MemberRow {
        name: enrollment.name.clone(),
        node_pub: node_pub_b64.to_string(),
        wrap_pub: b64_encode(wrap_pub),
        tags: enrollment.tags.clone(),
        admitted_at: now,
        signed: None,
        revoked_at: None,
        revoke_reason: None,
        revocation: None,
    })?;
    hub.store.remove_invite(&enrollment.code_id)?;
    hub.append_event(
        &enrollment.name,
        "node.enrolled",
        [
            (
                "invited_by",
                serde_json::Value::String(invite.inviter.clone()),
            ),
            ("version", serde_json::Value::String(hello.version.clone())),
        ],
    );
    hub.remember_enrollment(&enrollment.name, invite.wrapped_keys.clone());
    log(format!(
        "enrolled \"{}\" (invited by {})",
        enrollment.name, invite.inviter
    ));
    Ok(enrollment.name.clone())
}

fn bootstrap(
    hub: &Hub,
    hello: &Hello,
    auth: &Auth,
    node_pub_b64: &str,
    wrap_pub: &crypto::Key32,
) -> Result<String, HandshakeFailure> {
    let Some(enrollment) = &auth.enrollment else {
        return Err(reject(
            "invite_invalid",
            "Bootstrap needs the hub's bootstrap password.",
        ));
    };
    let invalid = || {
        reject(
            "invite_invalid",
            "That bootstrap password is not valid, or this World already has its first member.",
        )
    };
    if hub
        .store
        .meta_string(super::store::meta::BOOTSTRAP_USED)?
        .is_some()
        || hub.store.member_count()? > 0
    {
        return Err(invalid());
    }
    let Some(expected) = hub
        .store
        .meta_string(super::store::meta::BOOTSTRAP_ADMISSION)?
    else {
        return Err(invalid());
    };
    let token = b64_decode(&enrollment.admission_token).map_err(|_| invalid())?;
    if Invite::admission_of_token(&token) != expected {
        return Err(invalid());
    }
    crate::world::validate_node_name(&enrollment.name)
        .map_err(|error| reject("invalid_name", error))?;
    let now = crate::world::now_rfc3339();
    hub.store.put_member(&MemberRow {
        name: enrollment.name.clone(),
        node_pub: node_pub_b64.to_string(),
        wrap_pub: b64_encode(wrap_pub),
        tags: enrollment.tags.clone(),
        admitted_at: now,
        signed: None,
        revoked_at: None,
        revoke_reason: None,
        revocation: None,
    })?;
    hub.store
        .set_meta_string(super::store::meta::BOOTSTRAP_USED, "1")?;
    hub.append_event(
        &enrollment.name,
        "node.enrolled",
        [
            ("bootstrap", serde_json::Value::Bool(true)),
            ("version", serde_json::Value::String(hello.version.clone())),
        ],
    );
    hub.remember_enrollment(&enrollment.name, String::new());
    log(format!(
        "bootstrapped the World with \"{}\"",
        enrollment.name
    ));
    Ok(enrollment.name.clone())
}

async fn run_session<S>(hub: Arc<Hub>, mut link: Link<S>, peer: Peer, auth: Auth)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let address = link.peer;
    let name = peer.name.clone();
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    let cancel = CancellationToken::new();
    let replaced = hub.register(
        &name,
        Connected {
            generation: peer.generation,
            tx: tx.clone(),
            cancel: cancel.clone(),
        },
    );
    if replaced {
        log(format!(
            "\"{name}\" reconnected from {address}; the previous connection was replaced"
        ));
    }

    let session_row = match hub.store.update_session(&name, |row| {
        row.generation = peer.generation;
        row.last_seen = crate::world::now_rfc3339();
        row.version = peer.version.clone();
        row.protocol = peer.protocol;
    }) {
        Ok(row) => row,
        Err(error) => {
            log(format!("\"{name}\": cannot record the session: {error}"));
            hub.unregister(&name, peer.generation);
            link.close().await;
            return;
        }
    };
    let enrolled = hub.take_enrollment(&name).map(|wrapped_keys| Enrolled {
        wrapped_keys: (!wrapped_keys.is_empty()).then_some(wrapped_keys),
    });
    let welcome = Welcome {
        world_id: hub.world_id.clone(),
        name: name.clone(),
        hub_time: crate::world::now_rfc3339(),
        recv_seq: session_row.recv_seq,
        send_seq: session_row.send_seq,
        members_version: hub
            .store
            .meta_u64(super::store::meta::MEMBERS_VERSION)
            .unwrap_or(0),
        grant_version: hub
            .store
            .meta_u64(super::store::meta::GRANT_VERSION)
            .unwrap_or(0),
        key_epoch: hub
            .store
            .meta_u64(super::store::meta::KEY_EPOCH)
            .unwrap_or(0) as u32,
        rotation_pending: hub
            .store
            .meta_u64(super::store::meta::ROTATION_PENDING)
            .unwrap_or(0)
            > 0,
        enrolled,
    };
    if let Err(error) = link.send(&Frame::Welcome(welcome)).await {
        log(format!("\"{name}\": cannot send welcome: {error}"));
        hub.unregister(&name, peer.generation);
        return;
    }
    hub.append_event(
        &name,
        "node.online",
        [(
            "address",
            serde_json::Value::String(address.ip().to_string()),
        )],
    );
    log(format!("\"{name}\" online from {address}"));

    // Everything the member has not acknowledged goes out again, in order, before new traffic.
    match hub
        .store
        .outbox_ack(&name, auth.recv_seq)
        .and_then(|()| hub.store.outbox_after(&name, auth.recv_seq))
    {
        Ok(rows) => {
            for (seq, OutboxRow { id, env, .. }) in rows {
                let _ = tx.send(Frame::Msg {
                    seq: Some(seq),
                    id,
                    env,
                });
            }
        }
        Err(error) => log(format!("\"{name}\": cannot replay the outbox: {error}")),
    }

    let mut recv_seq = session_row.recv_seq;
    let mut last_heartbeat = Instant::now();
    let reason = loop {
        let deadline = tokio::time::sleep_until((last_heartbeat + HEARTBEAT_TIMEOUT).into());
        tokio::pin!(deadline);
        tokio::select! {
            () = cancel.cancelled() => break "replaced".to_string(),
            () = hub.shutdown.cancelled() => {
                let _ = link.send(&Frame::Bye { reason: "hub shutting down".to_string() }).await;
                break "hub_shutdown".to_string();
            }
            () = &mut deadline => {
                let _ = link.send(&Frame::Bye { reason: "heartbeat timeout".to_string() }).await;
                break "heartbeat_timeout".to_string();
            }
            outbound = rx.recv() => match outbound {
                Some(frame) => {
                    if let Err(error) = link.send(&frame).await {
                        break format!("write failed: {error}");
                    }
                }
                None => break "closed".to_string(),
            },
            inbound = link.recv() => match inbound {
                Ok(None) => break "connection_closed".to_string(),
                Err(error) => break format!("read failed: {error}"),
                Ok(Some(frame)) => match frame {
                    Frame::Heartbeat { seq, load } => {
                        last_heartbeat = Instant::now();
                        hub.note_heartbeat(&name, &load);
                        let _ = tx.send(Frame::HeartbeatAck { seq, hub_time: crate::world::now_rfc3339() });
                    }
                    Frame::Ack { seq } => {
                        if let Err(error) = hub.store.outbox_ack(&name, seq) {
                            log(format!("\"{name}\": cannot record an ack: {error}"));
                        }
                    }
                    Frame::Msg { seq: Some(seq), id, env } => {
                        if seq <= recv_seq {
                            let _ = tx.send(Frame::Ack { seq });
                        } else if seq == recv_seq + 1 {
                            match super::router::handle_reliable(&hub, &peer, id, env) {
                                Ok(()) => {}
                                Err(error) => {
                                    log(format!("\"{name}\": reliable message {seq} rejected: {error}"));
                                    hub.send_hub_error(&name, id, "rejected", &error);
                                }
                            }
                            recv_seq = seq;
                            if let Err(error) = hub.store.update_session(&name, |row| row.recv_seq = seq) {
                                log(format!("\"{name}\": cannot advance the receive cursor: {error}"));
                                break "store_failure".to_string();
                            }
                            let _ = tx.send(Frame::Ack { seq });
                        } else {
                            let _ = link.send(&Frame::Bye { reason: format!("sequence gap: expected {}, got {seq}", recv_seq + 1) }).await;
                            break "sequence_gap".to_string();
                        }
                    }
                    Frame::Msg { seq: None, id, env } => {
                        if let Err(error) = super::router::handle_request(&hub, &peer, id, env) {
                            log(format!("\"{name}\": request rejected: {error}"));
                            hub.send_hub_error(&name, id, "rejected", &error);
                        }
                    }
                    Frame::Bye { reason } => break format!("member said bye: {reason}"),
                    Frame::Hello(_) | Frame::Auth(_) => {
                        let _ = link.send(&Frame::Bye { reason: "handshake frame after authentication".to_string() }).await;
                        break "protocol_error".to_string();
                    }
                    Frame::Challenge(_) | Frame::Welcome(_) | Frame::Rejected(_) | Frame::HeartbeatAck { .. } | Frame::Replaced => {
                        let _ = link.send(&Frame::Bye { reason: "hub-only frame from a member".to_string() }).await;
                        break "protocol_error".to_string();
                    }
                }
            },
        }
    };
    if reason == "replaced" {
        let _ = link.send(&Frame::Replaced).await;
    }
    link.close().await;
    let still_current = hub.unregister(&name, peer.generation);
    let _ = hub
        .store
        .update_session(&name, |row| row.last_seen = crate::world::now_rfc3339());
    if still_current {
        hub.append_event(
            &name,
            "node.offline",
            [("reason", serde_json::Value::String(reason.clone()))],
        );
        hub.fail_pending_for_target(&name, "disconnected");
    }
    log(format!("\"{name}\" offline ({reason})"));
}

impl BindingMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Exporter => "exporter",
            Self::None => "none",
        }
    }
}
