//! The member's hub session: dial, authenticate, resync, heartbeat, dispatch, and reconnect
//! with jittered backoff. State lives in `WorldClient`; this task only owns the socket.

use super::client::{Inventory, LinkState, WorldClient};
use super::crypto::{b64_array, b64_decode, b64_encode};
use super::envelope::Envelope;
use super::identity::{Fingerprint, verify};
use super::link::{self, DialPlan, Dialed, Endpoint, Verify};
use super::messages::{self, kind};
use super::node::executor::Executor;
use super::node::log;
use super::wire::{self, Auth, BindingMode, Frame, Hello, Intent, Load, Welcome};
use super::{HUB_NAME, NetworkMode, PROTOCOL_VERSION, TlsMode};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const HEARTBEAT_MISSES: u32 = 3;
/// A heartbeat tick arriving this late means the machine slept.
const WAKE_GAP: Duration = Duration::from_secs(30);
const STABLE_AFTER: Duration = Duration::from_secs(60);
const MAX_BACKOFF_SECS: u64 = 60;
/// Consecutive replacements before the link pauses to stop two processes fighting.
const REPLACED_LIMIT: u32 = 3;
const REPLACED_PAUSE: Duration = Duration::from_secs(300);
const INVENTORY_INTERVAL: Duration = Duration::from_secs(300);

/// Runs the session until the client's shutdown token fires.
pub(crate) async fn run(client: Arc<WorldClient>, executor: Arc<Executor>) {
    let mut attempt: u32 = 0;
    loop {
        if client.shutdown.is_cancelled() {
            return;
        }
        client.update_link(|link| {
            link.state = LinkState::Connecting {
                attempt: attempt + 1,
            };
        });
        let started = Instant::now();
        match connect_once(&client, &executor).await {
            Ok(outcome) => {
                if started.elapsed() >= STABLE_AFTER {
                    attempt = 0;
                }
                match outcome {
                    SessionEnd::Replaced => {
                        let count = client
                            .with_state(|state| {
                                state.replaced_count += 1;
                                state.replaced_count
                            })
                            .unwrap_or(1);
                        client.update_link(|link| link.replaced_count = count);
                        log(format!(
                            "this connection was replaced by another process using the same World key ({count} time(s))"
                        ));
                        if count >= REPLACED_LIMIT {
                            pause(&client, "replaced by another process three times; another fastctx node may be running with this identity", REPLACED_PAUSE).await;
                            let _ = client.with_state(|state| state.replaced_count = 0);
                            client.update_link(|link| link.replaced_count = 0);
                            continue;
                        }
                    }
                    SessionEnd::Stopped(reason) => {
                        client.update_link(|link| {
                            link.state = LinkState::Stopped {
                                reason: reason.clone(),
                                until: None,
                            };
                            link.last_error = Some(reason.clone());
                        });
                        log(format!("link stopped: {reason}"));
                        client.wake.notified().await;
                        attempt = 0;
                        continue;
                    }
                    SessionEnd::Shutdown => return,
                    SessionEnd::Lost(reason) => log(format!("link lost: {reason}")),
                }
            }
            Err(error) => {
                client.update_link(|link| link.last_error = Some(error.clone()));
                if attempt == 0 {
                    log(format!("cannot reach the hub: {error}"));
                }
            }
        }
        attempt = attempt.saturating_add(1);
        let wait = backoff(attempt);
        client.update_link(|link| {
            link.state = LinkState::Reconnecting {
                attempt,
                next_attempt_at: super::format_rfc3339(
                    time::OffsetDateTime::now_utc()
                        + time::Duration::seconds(wait.as_secs() as i64),
                ),
            };
        });
        tokio::select! {
            () = client.shutdown.cancelled() => return,
            () = client.wake.notified() => {}
            () = tokio::time::sleep(wait) => {}
        }
    }
}

async fn pause(client: &WorldClient, reason: &str, duration: Duration) {
    client.update_link(|link| {
        link.state = LinkState::Stopped {
            reason: reason.to_string(),
            until: Some(super::format_rfc3339(
                time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(duration.as_secs() as i64),
            )),
        };
    });
    tokio::select! {
        () = client.shutdown.cancelled() => {}
        () = tokio::time::sleep(duration) => {}
    }
}

/// `rand(0, min(60, 2^(n-1)))` seconds; the first retry is immediate.
fn backoff(attempt: u32) -> Duration {
    if attempt <= 1 {
        return Duration::ZERO;
    }
    let ceiling = 2_u64
        .saturating_pow(attempt.saturating_sub(1).min(20))
        .min(MAX_BACKOFF_SECS);
    let random = super::crypto::random_bytes::<8>()
        .map(u64::from_le_bytes)
        .unwrap_or(0);
    Duration::from_millis((random % (ceiling * 1000 + 1)).max(250))
}

enum SessionEnd {
    Replaced,
    Stopped(String),
    Lost(String),
    Shutdown,
}

fn verify_mode(config: &super::WorldConfig) -> Verify {
    match config.tls {
        TlsMode::Pinned => Verify::Pinned(config.pinned_spki_sha256.clone().unwrap_or_default()),
        TlsMode::Webpki | TlsMode::Fronted => Verify::Webpki,
    }
}

async fn connect_once(
    client: &Arc<WorldClient>,
    executor: &Arc<Executor>,
) -> Result<SessionEnd, String> {
    let config = client.config.read().clone();
    let state = client.state_snapshot();
    let plan = DialPlan {
        endpoints: config
            .hub
            .iter()
            .map(|text| Endpoint::parse(text))
            .collect::<Result<Vec<_>, _>>()?,
        mode: config.network,
        interface: config.interface.clone(),
        preferred: state.last_network,
    };
    let verify = verify_mode(&config);
    let mut dialed = link::dial(&plan, &verify)
        .await
        .map_err(|failure| failure.to_string())?;
    let welcome = match tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        authenticate(client, &mut dialed, &config, state.recv_seq),
    )
    .await
    {
        Ok(Ok(welcome)) => welcome,
        Ok(Err(Handshake::Stopped(reason))) => {
            dialed.close().await;
            return Ok(SessionEnd::Stopped(reason));
        }
        Ok(Err(Handshake::Failed(error))) => {
            dialed.close().await;
            return Err(error);
        }
        Err(_) => {
            dialed.close().await;
            return Err("the hub did not finish the handshake within 10 s".to_string());
        }
    };
    let path = dialed.path.clone();
    let mode = path.mode();
    let _ = client.with_state(|state| state.last_network = Some(mode));
    client.update_link(|link| {
        link.state = LinkState::Connected;
        link.network = Some(mode);
        link.path = Some(path.describe());
        link.connected_since = Some(super::now_rfc3339());
        link.last_error = None;
        link.last_contact = Some(Instant::now());
        if let link::Path::Direct {
            interface, tunnels, ..
        } = &path
        {
            link.interface = Some(interface.clone());
            link.tunnels = tunnels.clone();
        } else {
            link.tunnels = Vec::new();
        }
        if let Ok(offset) = super::parse_rfc3339(&welcome.hub_time) {
            link.hub_time_offset_s =
                Some((offset - time::OffsetDateTime::now_utc()).whole_seconds());
        }
    });
    log(format!(
        "connected to {} as \"{}\" ({})",
        dialed.endpoint,
        welcome.name,
        path.describe()
    ));
    let outcome = run_connected(client, executor, dialed, welcome, mode).await;
    client.detach_sender();
    client.update_link(|link| {
        link.connected_since = None;
        if matches!(link.state, LinkState::Connected) {
            link.state = LinkState::Reconnecting {
                attempt: 0,
                next_attempt_at: super::now_rfc3339(),
            };
        }
    });
    Ok(outcome)
}

enum Handshake {
    Stopped(String),
    Failed(String),
}

impl From<String> for Handshake {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

async fn authenticate(
    client: &WorldClient,
    dialed: &mut Dialed,
    config: &super::WorldConfig,
    recv_seq: u64,
) -> Result<Welcome, Handshake> {
    let node_nonce = super::crypto::random_bytes::<32>()?;
    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        min_protocol: PROTOCOL_VERSION.saturating_sub(1).max(1),
        version: env!("CARGO_PKG_VERSION").to_string(),
        nonce: b64_encode(&node_nonce),
        node_pub: b64_encode(&client.identity.public_key()),
        wrap_pub: b64_encode(&client.identity.wrap_public()),
        intent: Intent::Auth,
    };
    dialed.send(&Frame::Hello(hello)).await?;
    let challenge = match dialed.recv().await? {
        Some(Frame::Challenge(challenge)) => challenge,
        Some(Frame::Rejected(rejected)) => return Err(rejection(rejected)),
        Some(_) => {
            return Err(Handshake::Failed(
                "the hub answered hello with something other than a challenge".to_string(),
            ));
        }
        None => {
            return Err(Handshake::Failed(
                "the hub closed the connection during the handshake".to_string(),
            ));
        }
    };
    let expected_key = Fingerprint::parse(&config.hub_key)?;
    let hub_pub = b64_array::<32>(&challenge.hub_pub, "hub public key")?;
    if Fingerprint::of(&hub_pub) != expected_key {
        return Err(Handshake::Stopped(format!(
            "hub_identity_mismatch: the hub at {} presented key {}, not the enrolled {expected_key}",
            dialed.endpoint,
            Fingerprint::of(&hub_pub)
        )));
    }
    let binding = match (challenge.binding, config.tls) {
        (BindingMode::Exporter, TlsMode::Fronted) => {
            return Err(Handshake::Stopped("hub_identity_mismatch: this member enrolled through a proxy (fronted) but the hub now binds to TLS directly; enroll again.".to_string()));
        }
        (BindingMode::None, TlsMode::Fronted) => Vec::new(),
        (BindingMode::None, _) => {
            return Err(Handshake::Stopped("hub_identity_mismatch: the hub disclaims the TLS channel binding this member enrolled with; a proxy may be in the way.".to_string()));
        }
        (BindingMode::Exporter, _) => dialed.binding.clone(),
    };
    let hub_nonce = b64_decode(&challenge.nonce)?;
    let node_pub = client.identity.public_key();
    let transcript = wire::hub_transcript(&node_nonce, &hub_nonce, &hub_pub, &node_pub, &binding);
    let signature = b64_decode(&challenge.sig)?;
    verify(&hub_pub, wire::HUB_HANDSHAKE_DOMAIN, &transcript, &signature)
        .map_err(|_| Handshake::Stopped("hub_identity_mismatch: the hub's challenge signature does not verify; the connection may be intercepted".to_string()))?;
    if challenge.world_id != config.world_id {
        return Err(Handshake::Stopped(format!(
            "hub_identity_mismatch: the hub serves World {} but this member belongs to {}",
            challenge.world_id, config.world_id
        )));
    }
    let node_transcript =
        wire::node_transcript(&hub_nonce, &node_nonce, &node_pub, &hub_pub, &binding);
    let auth = Auth {
        sig: b64_encode(
            &client
                .identity
                .sign(wire::NODE_HANDSHAKE_DOMAIN, &node_transcript),
        ),
        recv_seq,
        enrollment: None,
    };
    dialed.send(&Frame::Auth(auth)).await?;
    match dialed.recv().await? {
        Some(Frame::Welcome(welcome)) => Ok(welcome),
        Some(Frame::Rejected(rejected)) => Err(rejection(rejected)),
        Some(_) => Err(Handshake::Failed(
            "the hub answered auth with something other than welcome".to_string(),
        )),
        None => Err(Handshake::Failed(
            "the hub closed the connection after auth".to_string(),
        )),
    }
}

fn rejection(rejected: wire::Rejected) -> Handshake {
    match rejected.code.as_str() {
        "revoked" | "not_enrolled" | "protocol_mismatch" | "hub_identity_mismatch" => {
            Handshake::Stopped(format!("{}: {}", rejected.code, rejected.message))
        }
        _ => Handshake::Failed(format!("{}: {}", rejected.code, rejected.message)),
    }
}

async fn run_connected(
    client: &Arc<WorldClient>,
    executor: &Arc<Executor>,
    dialed: Dialed,
    welcome: Welcome,
    mode: NetworkMode,
) -> SessionEnd {
    let (mut sink, mut stream) = dialed.socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    client.attach_sender(tx.clone());

    // Resync: drop what the hub already has, resend what it lacks.
    match client.outbox_after(welcome.recv_seq) {
        Ok(entries) => {
            for (seq, entry) in entries {
                let _ = tx.send(Frame::reliable(seq, entry.env));
            }
        }
        Err(error) => log(format!("cannot replay the outbox: {error}")),
    }
    let name = client.name();
    let started = Instant::now();
    let after_welcome = {
        let client = Arc::clone(client);
        let welcome = welcome.clone();
        async move {
            if welcome.key_epoch > client.keys.read().current().epoch() {
                if let Err(error) = client.refresh_keys().await {
                    log(format!("key refresh failed: {error}"));
                }
            }
            if welcome.members_version != client.state_snapshot().members_version
                || client.members.read().members.is_empty()
            {
                if let Err(error) = client.refresh_members().await {
                    log(format!("member refresh failed: {error}"));
                }
            }
            if let Err(error) = client.refresh_inventories().await {
                log(format!("inventory refresh failed: {error}"));
            }
            let inventory = super::node::inventory::collect(&client).await;
            if let Err(error) = client.publish_record(&inventory) {
                log(format!("cannot publish the member record: {error}"));
            }
            if let Err(error) = client.publish_inventory(&inventory) {
                log(format!("cannot publish the inventory: {error}"));
            }
            if welcome.rotation_pending {
                match client.complete_rotation().await {
                    Ok(epoch) => log(format!(
                        "completed the pending World key rotation (epoch {epoch})"
                    )),
                    Err(error) => log(format!("cannot complete the pending key rotation: {error}")),
                }
            }
        }
    };
    tokio::pin!(after_welcome);
    let mut after_welcome_done = false;

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heartbeat_seq: u64 = 0;
    let mut last_ack_seq: u64 = 0;
    let mut last_tick = Instant::now();
    let mut heartbeat_sent_at: Option<Instant> = None;
    let mut recv_seq = client.state_snapshot().recv_seq;
    let mut network_fingerprint = link::netpath::scan().map(|view| view.fingerprint()).ok();
    let mut inventory_at = Instant::now();
    let mut last_direct_probe = Instant::now();
    let mut ack_deadline: Option<Instant> = None;

    let end = loop {
        tokio::select! {
            () = client.shutdown.cancelled() => {
                let _ = sink.send(Message::Binary(Frame::Bye { reason: "node shutting down".to_string() }.encode().unwrap_or_default().into())).await;
                break SessionEnd::Shutdown;
            }
            () = &mut after_welcome, if !after_welcome_done => {
                after_welcome_done = true;
            }
            outbound = rx.recv() => match outbound {
                Some(frame) => {
                    if matches!(frame, Frame::Msg { seq: Some(_), .. }) && ack_deadline.is_none() {
                        ack_deadline = Some(Instant::now() + super::client::ACK_TIMEOUT);
                    }
                    let bytes = match frame.encode() {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            log(format!("cannot encode an outbound frame: {error}"));
                            continue;
                        }
                    };
                    if let Err(error) = sink.send(Message::Binary(bytes.into())).await {
                        break SessionEnd::Lost(format!("write failed: {error}"));
                    }
                }
                None => break SessionEnd::Lost("outbound channel closed".to_string()),
            },
            _ = heartbeat.tick() => {
                let now = Instant::now();
                if now.duration_since(last_tick) > WAKE_GAP + HEARTBEAT_INTERVAL {
                    log("heartbeat tick arrived late; assuming the machine slept, reconnecting");
                    break SessionEnd::Lost("wake from sleep".to_string());
                }
                last_tick = now;
                if heartbeat_seq.saturating_sub(last_ack_seq) >= u64::from(HEARTBEAT_MISSES) {
                    break SessionEnd::Lost(format!("{HEARTBEAT_MISSES} heartbeats went unanswered"));
                }
                if let Some(deadline) = ack_deadline {
                    if now >= deadline {
                        break SessionEnd::Lost("a reliable message was not acknowledged within 15 s".to_string());
                    }
                }
                heartbeat_seq += 1;
                heartbeat_sent_at = Some(now);
                let link = client.link();
                let load = Load {
                    outbox_depth: link.outbox_depth as u32,
                    facts_version: client.state_snapshot().inventory_version,
                    rtt_ms: link.rtt_ms,
                    network: Some(mode.as_str().to_string()),
                    tls: Some(link.tls.as_str().to_string()),
                    ..Load::default()
                };
                let _ = tx.send(Frame::Heartbeat { seq: heartbeat_seq, load });
                client.update_link(|link| link.last_heartbeat_at = Some(super::now_rfc3339()));
                if let Ok(view) = link::netpath::scan() {
                    let fingerprint = view.fingerprint();
                    let changed = network_fingerprint.is_some_and(|previous| previous != fingerprint);
                    network_fingerprint = Some(fingerprint);
                    if changed && mode == NetworkMode::Direct {
                        log("the network interfaces changed; reselecting the path");
                        break SessionEnd::Lost("network change".to_string());
                    }
                }
                if inventory_at.elapsed() >= INVENTORY_INTERVAL {
                    inventory_at = now;
                    let client = Arc::clone(client);
                    tokio::spawn(async move {
                        let inventory: Inventory = super::node::inventory::collect(&client).await;
                        if let Err(error) = client.publish_inventory(&inventory) {
                            log(format!("cannot publish the inventory: {error}"));
                        }
                    });
                }
                if mode == NetworkMode::System
                    && client.config.read().network == NetworkMode::Auto
                    && last_direct_probe.elapsed() >= link::DIRECT_REPROBE_INTERVAL
                {
                    last_direct_probe = now;
                    let config = client.config.read().clone();
                    let plan = DialPlan {
                        endpoints: config.hub.iter().filter_map(|text| Endpoint::parse(text).ok()).collect(),
                        mode: NetworkMode::Direct,
                        interface: config.interface.clone(),
                        preferred: None,
                    };
                    if let Ok(mut probe) = link::dial_direct(&plan, &verify_mode(&config)).await {
                        probe.close().await;
                        let _ = client.with_state(|state| state.last_network = Some(NetworkMode::Direct));
                        log("the direct path is available again; the next reconnect will use it");
                    }
                }
            }
            inbound = stream.next() => {
                let frame = match inbound {
                    None => break SessionEnd::Lost("the hub closed the connection".to_string()),
                    Some(Err(error)) => break SessionEnd::Lost(format!("read failed: {error}")),
                    Some(Ok(Message::Binary(bytes))) => match Frame::decode(&bytes) {
                        Ok(frame) => frame,
                        Err(error) => {
                            log(format!("dropping an unreadable frame: {error}"));
                            continue;
                        }
                    },
                    Some(Ok(Message::Text(text))) => match Frame::decode(text.as_bytes()) {
                        Ok(frame) => frame,
                        Err(error) => {
                            log(format!("dropping an unreadable frame: {error}"));
                            continue;
                        }
                    },
                    Some(Ok(Message::Close(_))) => break SessionEnd::Lost("the hub closed the connection".to_string()),
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                        continue;
                    }
                    Some(Ok(_)) => continue,
                };
                client.update_link(|link| link.last_contact = Some(Instant::now()));
                match frame {
                    Frame::HeartbeatAck { seq, hub_time } => {
                        last_ack_seq = last_ack_seq.max(seq);
                        let rtt = heartbeat_sent_at.map(|sent| sent.elapsed().as_millis() as u32);
                        client.update_link(|link| {
                            link.last_ack_at = Some(super::now_rfc3339());
                            if let Some(rtt) = rtt {
                                link.rtt_ms = Some(match link.rtt_ms {
                                    Some(previous) => (previous * 7 + rtt) / 8,
                                    None => rtt,
                                });
                            }
                            if let Ok(hub_time) = super::parse_rfc3339(&hub_time) {
                                link.hub_time_offset_s = Some((hub_time - time::OffsetDateTime::now_utc()).whole_seconds());
                            }
                        });
                    }
                    Frame::Ack { seq } => {
                        ack_deadline = None;
                        if let Err(error) = client.outbox_ack(seq) {
                            log(format!("cannot drop an acknowledged message: {error}"));
                        }
                    }
                    Frame::Msg { seq: Some(seq), id, env } => {
                        if seq <= recv_seq {
                            let _ = tx.send(Frame::Ack { seq });
                        } else if seq == recv_seq + 1 {
                            handle_reliable(client, executor, id, env).await;
                            recv_seq = seq;
                            if let Err(error) = client.with_state(|state| state.recv_seq = seq) {
                                log(format!("cannot advance the receive cursor: {error}"));
                            }
                            let _ = tx.send(Frame::Ack { seq });
                        } else {
                            break SessionEnd::Lost(format!("sequence gap from the hub: expected {}, got {seq}", recv_seq + 1));
                        }
                    }
                    Frame::Msg { seq: None, id, env } => {
                        handle_request_frame(client, executor, id, env);
                    }
                    Frame::Replaced => break SessionEnd::Replaced,
                    Frame::Bye { reason } => break SessionEnd::Lost(format!("the hub said bye: {reason}")),
                    Frame::Rejected(rejected) => break SessionEnd::Stopped(format!("{}: {}", rejected.code, rejected.message)),
                    Frame::Hello(_) | Frame::Challenge(_) | Frame::Auth(_) | Frame::Welcome(_) | Frame::Heartbeat { .. } => {
                        log("the hub sent a handshake frame after welcome; ignoring it");
                    }
                }
            }
        }
    };
    let _ = started;
    let _ = name;
    end
}

/// Applies a reliable message from the hub or, through it, from another member.
async fn handle_reliable(
    client: &Arc<WorldClient>,
    executor: &Arc<Executor>,
    _id: Option<u64>,
    env: Envelope,
) {
    let opened = match env.open(Some(&client.keys.read())) {
        Ok(opened) => opened,
        Err(error) => {
            log(format!(
                "dropping a reliable message this member cannot open: {error}"
            ));
            return;
        }
    };
    let header = &opened.header;
    if !opened.encrypted && !messages::plaintext_allowed(&header.t, &header.from) {
        log(format!(
            "dropping a plaintext {} from \"{}\"",
            header.t, header.from
        ));
        return;
    }
    if header.from != HUB_NAME {
        if let Err(error) = client.accept_counter(&header.from, header.n) {
            log(format!("dropping {}: {error}", header.t));
            return;
        }
    }
    // Anything that asks the hub a question runs off the read loop: the answer arrives
    // through this very loop, so awaiting it here would deadlock until the request timed out.
    match header.t.as_str() {
        kind::MEMBERS_CHANGED => {
            let client = Arc::clone(client);
            tokio::spawn(async move {
                if let Err(error) = client.refresh_members().await {
                    log(format!("member refresh failed: {error}"));
                }
                if let Err(error) = client.refresh_inventories().await {
                    log(format!("inventory refresh failed: {error}"));
                }
            });
        }
        kind::GRANT_SYNC => {
            match messages::decode::<messages::GrantSync>(&opened.body, kind::GRANT_SYNC) {
                Ok(sync) => match client.apply_grant_sync(sync) {
                    Ok(rejected) if rejected.is_empty() => {}
                    Ok(rejected) => log(format!(
                        "grants ignored because they did not verify: {}",
                        rejected.join("; ")
                    )),
                    Err(error) => log(format!("cannot apply grants: {error}")),
                },
                Err(error) => log(error),
            }
        }
        kind::REVOKED => match messages::decode::<messages::Revoked>(&opened.body, kind::REVOKED) {
            Ok(revoked) if revoked.name == client.name() => {
                client.update_link(|link| {
                    link.state = LinkState::Stopped {
                        reason: format!(
                            "revoked: this member was removed from the World ({})",
                            revoked.reason
                        ),
                        until: None,
                    };
                });
                log("this member was revoked from the World");
            }
            Ok(_) => {
                let client = Arc::clone(client);
                tokio::spawn(async move {
                    if let Err(error) = client.refresh_members().await {
                        log(format!("member refresh failed: {error}"));
                    }
                });
            }
            Err(error) => log(error),
        },
        kind::KEY_ROTATED => {
            let client = Arc::clone(client);
            tokio::spawn(async move {
                if let Err(error) = client.refresh_keys().await {
                    log(format!("key refresh failed: {error}"));
                }
            });
        }
        kind::HUB_ERROR => {
            match messages::decode::<messages::HubError>(&opened.body, kind::HUB_ERROR) {
                Ok(error) => log(format!(
                    "the hub rejected a message: {}: {}",
                    error.code, error.message
                )),
                Err(error) => log(error),
            }
        }
        kind::CALL => executor.spawn_call(None, opened),
        other => log(format!(
            "ignoring an unsupported reliable message \"{other}\" from \"{}\"",
            header.from
        )),
    }
}

/// Routes a request frame: requests addressed to this member go to the executor, answers go
/// to whoever is waiting for them.
fn handle_request_frame(
    client: &Arc<WorldClient>,
    executor: &Arc<Executor>,
    id: Option<u64>,
    env: Envelope,
) {
    let opened = match env.open(Some(&client.keys.read())) {
        Ok(opened) => opened,
        Err(error) => {
            log(format!(
                "dropping a request this member cannot open: {error}"
            ));
            return;
        }
    };
    let header = opened.header.clone();
    if !opened.encrypted && !messages::plaintext_allowed(&header.t, &header.from) {
        log(format!(
            "dropping a plaintext {} from \"{}\"",
            header.t, header.from
        ));
        return;
    }
    match header.t.as_str() {
        kind::CALL => {
            if let Err(error) = client.accept_counter(&header.from, header.n) {
                log(format!("dropping a call: {error}"));
                return;
            }
            executor.spawn_call(id, opened);
        }
        kind::CANCEL => {
            if let Some(id) = id {
                executor.cancel(id);
            }
        }
        _ => {
            let Some(id) = id else {
                log(format!("dropping an answer without an id ({})", header.t));
                return;
            };
            if !client.deliver_answer(id, opened) {
                log(format!(
                    "no request is waiting for answer {id} ({})",
                    header.t
                ));
            }
        }
    }
}
