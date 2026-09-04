//! `fastctx hub`: the one process in a World that listens. It admits members, keeps the
//! member table, grants, sealed keys, inventories, and the event log, and routes envelopes it
//! cannot read. It never executes anything.

pub(crate) mod http;
pub(crate) mod router;
pub(crate) mod session;
pub(crate) mod store;
pub(crate) mod tls;

use crate::world::crypto;
use crate::world::envelope::Envelope;
use crate::world::grant::GrantSet;
use crate::world::identity::SigningIdentity;
use crate::world::messages::{self, kind};
use crate::world::wire::{BindingMode, Frame, Load};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use store::Store;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Reserved `n` counter block written to the store at once; the hub burns at most this many
/// values on a crash.
const N_RESERVATION: u64 = 1_000;
/// A pending request older than this is dropped; its caller timed out long ago.
const PENDING_MAX_AGE: Duration = Duration::from_secs(600);
const TLS_ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_INTERVAL: Duration = Duration::from_secs(10);
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(1);

/// Everything `fastctx hub serve` needs to start.
#[derive(Clone, Debug)]
pub(crate) struct HubOptions {
    pub(crate) listen: String,
    pub(crate) data: PathBuf,
    pub(crate) cert: Option<PathBuf>,
    pub(crate) key: Option<PathBuf>,
    /// Serve plain HTTP for a reverse proxy that terminates TLS; the channel binding is empty.
    pub(crate) behind_proxy: bool,
    /// Discard the bootstrap password and print a fresh one.
    pub(crate) reset_bootstrap: bool,
}

/// What `fastctx hub status` reads; refreshed every ten seconds while the hub runs.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct HubStatus {
    pub(crate) version: String,
    pub(crate) pid: u32,
    pub(crate) written_at: String,
    pub(crate) started_at: String,
    pub(crate) listen: String,
    pub(crate) world_id: String,
    pub(crate) hub_key: String,
    pub(crate) tls: String,
    pub(crate) binding: String,
    pub(crate) members: Vec<MemberStatus>,
    pub(crate) open_invites: u64,
    pub(crate) events: u64,
    pub(crate) rotation_pending: bool,
    pub(crate) bootstrap_used: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MemberStatus {
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) last_seen: String,
    pub(crate) outbox: u64,
    pub(crate) version: String,
}

pub(crate) struct Connected {
    pub(crate) generation: u64,
    pub(crate) tx: mpsc::UnboundedSender<Frame>,
    pub(crate) cancel: CancellationToken,
    pub(crate) since: Instant,
}

struct Pending {
    caller: String,
    caller_id: u64,
    target: String,
    created: Instant,
}

pub(crate) struct Hub {
    pub(crate) store: Store,
    pub(crate) identity: SigningIdentity,
    pub(crate) world_id: String,
    pub(crate) binding_mode: BindingMode,
    pub(crate) shutdown: CancellationToken,
    pub(crate) data_dir: PathBuf,
    listen: String,
    tls_description: String,
    started: Instant,
    started_at: String,
    sessions: Mutex<HashMap<String, Connected>>,
    generations: Mutex<HashMap<String, u64>>,
    pending: Mutex<HashMap<u64, Pending>>,
    enrollments: Mutex<HashMap<String, String>>,
    next_hub_id: AtomicU64,
    next_n: AtomicU64,
    n_reserved_until: Mutex<u64>,
    grants: Mutex<GrantSet>,
}

pub(crate) fn log(message: impl std::fmt::Display) {
    eprintln!("[{}] hub: {message}", crate::world::now_rfc3339());
}

/// Runs the hub until interrupted.
pub(crate) async fn run(options: HubOptions) -> Result<(), String> {
    crate::edit::private_storage::ensure_private_directory(&options.data, "hub data")?;
    let store = Store::open(&options.data.join("hub.redb"))?;
    let identity = load_or_create_identity(&options.data.join("hub.key"))?;
    let world_id = match store.meta_string(store::meta::WORLD_ID)? {
        Some(id) => id,
        None => {
            let id = format!("w-{}", &hex::encode(crypto::random_bytes::<4>()?));
            store.set_meta_string(store::meta::WORLD_ID, &id)?;
            id
        }
    };
    let bootstrap_password = prepare_bootstrap(&store, options.reset_bootstrap)?;

    let tls = if options.behind_proxy {
        if options.cert.is_some() || options.key.is_some() {
            return Err(
                "--behind-proxy serves plain HTTP; --cert and --key do not apply.".to_string(),
            );
        }
        None
    } else {
        Some(tls::prepare(
            &options.data,
            options.cert.as_deref(),
            options.key.as_deref(),
        )?)
    };
    let listener = tokio::net::TcpListener::bind(&options.listen)
        .await
        .map_err(|error| format!("Cannot listen on {}: {error}", options.listen))?;
    let local = listener
        .local_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| options.listen.clone());

    let n_start = store.meta_u64(store::meta::HUB_N)?;
    store.set_meta_u64(store::meta::HUB_N, n_start + N_RESERVATION)?;
    let hub = Arc::new(Hub {
        store,
        identity,
        world_id: world_id.clone(),
        binding_mode: if tls.is_some() {
            BindingMode::Exporter
        } else {
            BindingMode::None
        },
        shutdown: CancellationToken::new(),
        data_dir: options.data.clone(),
        listen: local.clone(),
        tls_description: tls.as_ref().map_or_else(
            || "plain HTTP behind a reverse proxy".to_string(),
            tls::ServerTls::describe,
        ),
        started: Instant::now(),
        started_at: crate::world::now_rfc3339(),
        sessions: Mutex::new(HashMap::new()),
        generations: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
        enrollments: Mutex::new(HashMap::new()),
        next_hub_id: AtomicU64::new(1),
        next_n: AtomicU64::new(n_start + 1),
        n_reserved_until: Mutex::new(n_start + N_RESERVATION),
        grants: Mutex::new(GrantSet::default()),
    });
    hub.reload_grants();

    log(format!(
        "World {world_id}, hub key {}",
        hub.identity.fingerprint()
    ));
    log(format!("listening on {local} ({})", hub.tls_description));
    if let Some(password) = bootstrap_password {
        eprintln!();
        eprintln!("This World has no member yet. On the first machine, run:");
        eprintln!();
        eprintln!(
            "    fastctx world init <this hub's host:port> --bootstrap {password} --name <machine-name>"
        );
        eprintln!();
        eprintln!(
            "The password is shown once and works once. Restart with --reset-bootstrap to get another."
        );
        eprintln!();
    }
    hub.write_status();

    let status_hub = Arc::clone(&hub);
    let status_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = status_hub.shutdown.cancelled() => return,
                () = tokio::time::sleep(STATUS_INTERVAL) => status_hub.write_status(),
            }
        }
    });
    let maintenance_hub = Arc::clone(&hub);
    let maintenance_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = maintenance_hub.shutdown.cancelled() => return,
                () = tokio::time::sleep(MAINTENANCE_INTERVAL) => maintenance_hub.maintain(),
            }
        }
    });

    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            () = wait_for_shutdown_signal() => {
                log("shutdown requested");
                break;
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    configure_socket(&stream);
                    let hub = Arc::clone(&hub);
                    let tls = tls.clone();
                    connections.spawn(async move {
                        match tls {
                            Some(tls) => {
                                let accepted = tokio::time::timeout(TLS_ACCEPT_TIMEOUT, tls.acceptor.accept(stream)).await;
                                match accepted {
                                    Ok(Ok(stream)) => match exporter_binding(stream.get_ref().1) {
                                        Ok(binding) => http::serve_connection(hub, stream, binding, peer).await,
                                        Err(error) => log(format!("{peer}: {error}")),
                                    },
                                    Ok(Err(error)) => log(format!("{peer}: TLS handshake failed: {error}")),
                                    Err(_) => log(format!("{peer}: TLS handshake timed out")),
                                }
                            }
                            None => http::serve_connection(hub, stream, Vec::new(), peer).await,
                        }
                    });
                }
                Err(error) => {
                    log(format!("accept failed: {error}; retrying"));
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            },
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                if let Err(error) = result {
                    log(format!("connection task failed: {error}"));
                }
            }
        }
    }
    hub.shutdown.cancel();
    let deadline = tokio::time::sleep(Duration::from_secs(3));
    tokio::pin!(deadline);
    while !connections.is_empty() {
        tokio::select! {
            _ = &mut deadline => {
                connections.abort_all();
                break;
            }
            _ = connections.join_next() => {}
        }
    }
    status_task.abort();
    maintenance_task.abort();
    hub.write_status();
    let _ = std::fs::remove_file(hub.data_dir.join("status.json"));
    Ok(())
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(terminate) => terminate,
            Err(_) => return std::future::pending::<()>().await,
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn configure_socket(stream: &tokio::net::TcpStream) {
    let socket = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new().with_time(Duration::from_secs(30));
    let _ = socket.set_tcp_keepalive(&keepalive);
    let _ = socket.set_tcp_nodelay(true);
}

/// RFC 9266 `tls-exporter` binding for a completed TLS 1.3 connection.
pub(crate) fn exporter_binding(connection: &rustls::ServerConnection) -> Result<Vec<u8>, String> {
    connection
        .export_keying_material(
            vec![0_u8; crate::world::wire::BINDING_LEN],
            crate::world::wire::EXPORTER_LABEL,
            None,
        )
        .map_err(|error| format!("cannot export the TLS channel binding: {error}"))
}

fn load_or_create_identity(path: &Path) -> Result<SigningIdentity, String> {
    match SigningIdentity::load(path)? {
        Some(identity) => Ok(identity),
        None => {
            let identity = SigningIdentity::generate()?;
            identity.save(path)?;
            Ok(identity)
        }
    }
}

/// Returns the bootstrap password to print when the World still has no member.
fn prepare_bootstrap(store: &Store, reset: bool) -> Result<Option<String>, String> {
    if reset {
        store.remove_meta(store::meta::BOOTSTRAP_ADMISSION)?;
        store.remove_meta(store::meta::BOOTSTRAP_USED)?;
    }
    if store.meta_string(store::meta::BOOTSTRAP_USED)?.is_some() || store.member_count()? > 0 {
        return Ok(None);
    }
    if !reset
        && store
            .meta_string(store::meta::BOOTSTRAP_ADMISSION)?
            .is_some()
    {
        eprintln!(
            "The bootstrap password printed at first start is still valid. Lost it? Restart with --reset-bootstrap."
        );
        return Ok(None);
    }
    let secret = crypto::random_bytes::<24>()?;
    let password = crypto::b64url_encode(&secret);
    let token = crypto::hmac_sha256(password.as_bytes(), b"admission");
    store.set_meta_string(
        store::meta::BOOTSTRAP_ADMISSION,
        &hex::encode(crypto::sha256(&token)),
    )?;
    Ok(Some(password))
}

impl Hub {
    pub(crate) fn next_generation(&self, name: &str) -> u64 {
        let mut generations = self.generations.lock();
        let entry = generations.entry(name.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }

    /// Records the live connection for `name`, replacing and cancelling an older one.
    pub(crate) fn register(&self, name: &str, connection: Connected) -> bool {
        let previous = self.sessions.lock().insert(name.to_string(), connection);
        match previous {
            Some(previous) => {
                previous.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Removes the connection if it is still the current one; returns whether it was.
    pub(crate) fn unregister(&self, name: &str, generation: u64) -> bool {
        let mut sessions = self.sessions.lock();
        match sessions.get(name) {
            Some(current) if current.generation == generation => {
                sessions.remove(name);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn is_online(&self, name: &str) -> bool {
        self.sessions.lock().contains_key(name)
    }

    fn connection(&self, name: &str) -> Option<ConnectionHandle> {
        self.sessions
            .lock()
            .get(name)
            .map(|connection| ConnectionHandle {
                tx: connection.tx.clone(),
                cancel: connection.cancel.clone(),
            })
    }

    pub(crate) fn send_to_online(&self, name: &str, frame: Frame) -> bool {
        match self.connection(name) {
            Some(connection) => connection.tx.send(frame).is_ok(),
            None => false,
        }
    }

    pub(crate) fn last_seen_text(&self, name: &str) -> String {
        self.store
            .session(name)
            .ok()
            .map(|row| row.last_seen)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "never".to_string())
    }

    pub(crate) fn note_heartbeat(&self, name: &str, load: &Load) {
        if let Err(error) = self.store.update_session(name, |row| {
            row.last_seen = crate::world::now_rfc3339();
            row.rtt_ms = load.rtt_ms;
            if load.network.is_some() {
                row.network = load.network.clone();
            }
            if load.tls.is_some() {
                row.tls = load.tls.clone();
            }
        }) {
            log(format!("\"{name}\": cannot record a heartbeat: {error}"));
        }
    }

    /// Queues a reliable message for `to` and pushes it if `to` is online.
    pub(crate) fn queue_reliable(
        &self,
        to: &str,
        env: Envelope,
        id: Option<u64>,
    ) -> Result<u64, String> {
        let seq = self
            .store
            .outbox_push(to, &env, id, &crate::world::now_rfc3339())?;
        self.send_to_online(
            to,
            Frame::Msg {
                seq: Some(seq),
                id,
                env,
            },
        );
        Ok(seq)
    }

    pub(crate) fn register_pending(&self, caller: &str, caller_id: u64, target: &str) -> u64 {
        let hub_id = self.next_hub_id.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().insert(
            hub_id,
            Pending {
                caller: caller.to_string(),
                caller_id,
                target: target.to_string(),
                created: Instant::now(),
            },
        );
        hub_id
    }

    pub(crate) fn forget_pending(&self, hub_id: u64) {
        self.pending.lock().remove(&hub_id);
    }

    /// Forwards a target's answer to the caller that asked.
    pub(crate) fn complete_pending(&self, hub_id: u64, from: &str, env: Envelope) {
        let Some(pending) = self.pending.lock().remove(&hub_id) else {
            return;
        };
        if pending.target != from {
            log(format!(
                "\"{from}\" answered request {hub_id} that belongs to \"{}\"; dropped",
                pending.target
            ));
            return;
        }
        self.send_to_online(&pending.caller, Frame::request(pending.caller_id, env));
    }

    /// Relays a caller's cancel to every target still holding its request. The relayed cancel
    /// is hub-originated and carries the hub-side request id the target knows.
    pub(crate) fn cancel_pending(&self, caller: &str, caller_id: Option<u64>) {
        let Some(caller_id) = caller_id else {
            return;
        };
        let targets = self
            .pending
            .lock()
            .iter()
            .filter(|(_, pending)| pending.caller == caller && pending.caller_id == caller_id)
            .map(|(hub_id, pending)| (*hub_id, pending.target.clone()))
            .collect::<Vec<_>>();
        for (hub_id, target) in targets {
            if let Ok(cancel) =
                self.hub_envelope(kind::CANCEL, &target, &messages::HubResult::default())
            {
                self.send_to_online(
                    &target,
                    Frame::Msg {
                        seq: None,
                        id: Some(hub_id),
                        env: cancel,
                    },
                );
            }
        }
    }

    /// Answers every request waiting on `target` with a status, because it left.
    pub(crate) fn fail_pending_for_target(&self, target: &str, status: &str) {
        let failed = {
            let mut pending = self.pending.lock();
            let ids = pending
                .iter()
                .filter(|(_, entry)| entry.target == target)
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| pending.remove(&id))
                .collect::<Vec<_>>()
        };
        for entry in failed {
            self.answer(
                &entry.caller,
                entry.caller_id,
                kind::CALL_STATUS,
                &messages::CallStatus {
                    node: target.to_string(),
                    status: status.to_string(),
                    message: format!("Node \"{target}\" disconnected before answering."),
                },
            );
        }
    }

    pub(crate) fn remember_enrollment(&self, name: &str, wrapped_keys: String) {
        self.enrollments
            .lock()
            .insert(name.to_string(), wrapped_keys);
    }

    pub(crate) fn take_enrollment(&self, name: &str) -> Option<String> {
        self.enrollments.lock().remove(name)
    }

    /// The hub's own monotonic envelope counter, reserved in blocks so it survives restarts.
    pub(crate) fn next_n(&self) -> u64 {
        let n = self.next_n.fetch_add(1, Ordering::Relaxed);
        let mut reserved = self.n_reserved_until.lock();
        if n + 1 >= *reserved {
            let next_reservation = *reserved + N_RESERVATION;
            if let Err(error) = self
                .store
                .set_meta_u64(store::meta::HUB_N, next_reservation)
            {
                log(format!("cannot reserve envelope counters: {error}"));
            } else {
                *reserved = next_reservation;
            }
        }
        n
    }

    fn maintain(&self) {
        let now = time::OffsetDateTime::now_utc();
        match self.store.expire_invites(now) {
            Ok(0) | Err(_) => {}
            Ok(removed) => log(format!("expired {removed} invite(s)")),
        }
        let stale = {
            let mut pending = self.pending.lock();
            let ids = pending
                .iter()
                .filter(|(_, entry)| entry.created.elapsed() > PENDING_MAX_AGE)
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            ids.into_iter().filter_map(|id| pending.remove(&id)).count()
        };
        if stale > 0 {
            log(format!("dropped {stale} stale pending request(s)"));
        }
        self.process_control_requests();
    }

    /// Applies operator requests written by `fastctx hub revoke` while the hub runs.
    fn process_control_requests(&self) {
        let directory = self.data_dir.join("control");
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(stem) = file_name.strip_suffix(".request") else {
                continue;
            };
            let result = match std::fs::read(&path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    serde_json::from_slice::<ControlRequest>(&bytes)
                        .map_err(|error| error.to_string())
                }) {
                Ok(ControlRequest::Revoke { name }) => self
                    .revoke(&name, "operator", "revoked")
                    .map(|()| format!("\"{name}\" revoked.")),
                Err(error) => Err(format!("unreadable control request: {error}")),
            };
            let response = ControlResponse {
                ok: result.is_ok(),
                message: result.unwrap_or_else(|error| error),
            };
            let _ = std::fs::remove_file(&path);
            let _ = crate::world::write_atomic(
                &directory.join(format!("{stem}.result")),
                &serde_json::to_vec(&response).unwrap_or_default(),
            );
        }
    }

    pub(crate) fn write_status(&self) {
        let sessions = self.store.sessions().unwrap_or_default();
        let members = self
            .store
            .members()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| !row.is_revoked())
            .map(|row| {
                let session = sessions.get(&row.name).cloned().unwrap_or_default();
                MemberStatus {
                    state: if self.is_online(&row.name) {
                        "online"
                    } else {
                        "offline"
                    }
                    .to_string(),
                    outbox: self.store.outbox_depth(&row.name).unwrap_or(0),
                    last_seen: session.last_seen,
                    version: session.version,
                    name: row.name,
                }
            })
            .collect();
        let status = HubStatus {
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            written_at: crate::world::now_rfc3339(),
            started_at: self.started_at.clone(),
            listen: self.listen.clone(),
            world_id: self.world_id.clone(),
            hub_key: self.identity.fingerprint().to_string(),
            tls: self.tls_description.clone(),
            binding: self.binding_mode.as_str().to_string(),
            members,
            open_invites: self.store.invite_count().unwrap_or(0),
            events: self.store.event_count().unwrap_or(0),
            rotation_pending: self
                .store
                .meta_u64(store::meta::ROTATION_PENDING)
                .unwrap_or(0)
                > 0,
            bootstrap_used: self
                .store
                .meta_string(store::meta::BOOTSTRAP_USED)
                .ok()
                .flatten()
                .is_some(),
        };
        if let Ok(json) = serde_json::to_vec_pretty(&status) {
            let _ = crate::world::write_atomic(&self.data_dir.join("status.json"), &json);
        }
        let _ = self.started;
    }
}

struct ConnectionHandle {
    tx: mpsc::UnboundedSender<Frame>,
    cancel: CancellationToken,
}

/// An operator request dropped into `<data>/control/` for the running hub.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum ControlRequest {
    Revoke { name: String },
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ControlResponse {
    pub(crate) ok: bool,
    pub(crate) message: String,
}

/// Reads the status file a running hub maintains; `Ok(None)` when no hub wrote one.
pub(crate) fn read_status(data: &Path) -> Result<Option<HubStatus>, String> {
    let Some(bytes) = crate::world::read_optional(&data.join("status.json"))? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("Cannot parse the hub status file: {error}"))
}

/// Whether a status file was written recently enough to belong to a live hub.
pub(crate) fn status_is_live(status: &HubStatus) -> bool {
    crate::world::parse_rfc3339(&status.written_at)
        .map(|written| time::OffsetDateTime::now_utc() - written < time::Duration::seconds(45))
        .unwrap_or(false)
}

/// Asks a running hub to revoke a member, or edits the store directly when no hub runs.
pub(crate) fn revoke_from_cli(data: &Path, name: &str) -> Result<String, String> {
    crate::world::validate_node_name(name)?;
    let live = read_status(data)?.is_some_and(|status| status_is_live(&status));
    if live {
        let directory = data.join("control");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("Cannot create the hub control directory: {error}"))?;
        let stem = format!("revoke-{name}-{}", std::process::id());
        let request = directory.join(format!("{stem}.request"));
        let result = directory.join(format!("{stem}.result"));
        crate::world::write_atomic(
            &request,
            &serde_json::to_vec(&ControlRequest::Revoke {
                name: name.to_string(),
            })
            .map_err(|error| error.to_string())?,
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(bytes) = crate::world::read_optional(&result)? {
                let _ = std::fs::remove_file(&result);
                let response: ControlResponse = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("Cannot parse the hub's answer: {error}"))?;
                return if response.ok {
                    Ok(response.message)
                } else {
                    Err(response.message)
                };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_file(&request);
        return Err("The hub is running but did not answer within 5 seconds.".to_string());
    }
    let store = Store::open(&data.join("hub.redb"))?;
    let Some(mut row) = store.member(name)? else {
        return Err(format!("No member named \"{name}\"."));
    };
    if row.is_revoked() {
        return Ok(format!("\"{name}\" was already revoked."));
    }
    row.revoked_at = Some(crate::world::now_rfc3339());
    row.revoke_reason = Some("revoked".to_string());
    store.put_member(&row)?;
    store.remove_sealed_keys_for(name)?;
    store.remove_inventory(name)?;
    store.outbox_clear(name)?;
    store.set_meta_u64(store::meta::ROTATION_PENDING, 1)?;
    let mut facts = std::collections::BTreeMap::new();
    facts.insert(
        "by".to_string(),
        serde_json::Value::String("operator".to_string()),
    );
    facts.insert(
        "reason".to_string(),
        serde_json::Value::String("revoked".to_string()),
    );
    store.append_event(name, "node.revoked", facts, None)?;
    Ok(format!(
        "\"{name}\" revoked (the hub was not running; members learn of it when it starts)."
    ))
}
