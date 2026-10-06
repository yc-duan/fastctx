//! The hub: the one machine every other machine connects to.
//!
//! Only the hub listens; every machine dials out to it and keeps one
//! control connection open, so machines behind home routers, campus
//! networks and company firewalls need no open port. The hub passes calls
//! between connected machines, keeps letters for machines that are away,
//! pairs the two ends of each data transfer, and keeps the list of members
//! (`world-network.md`).
//!
//! A relay is the hub's forwarding layer without its store (`relay`): it
//! carries the sessions of the machines below it over one relay session
//! with its own upstream (`attached`), and pairs the data connections of
//! two machines both below it itself.
//!
//! The hub reads only what it needs to route: what members say to each
//! other is sealed for the member it is for, and signed by the member that
//! says it, so the hub can neither read it nor speak for anyone; every
//! machine checks the signature and the list of members itself before
//! acting on anything (`fastctx_net::trust`).

mod attached;
mod control;
mod data;
mod join;
mod ledger;
mod ledger_query;
mod log;
pub mod relay;
mod store;

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use fastctx_net::code::Code;
use fastctx_net::sign::{Domain, PublicKey, Secret, Signature};
use fastctx_net::tls::HubCertificate;
use fastctx_net::trust::{CertPin, WorldId};
use fastctx_net::wire::{self, Down, Intent, PROTOCOL, Refusal, SILENCE, Up};
use fastctx_types::MachineName;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

pub use store::HubFile;
use store::Store;

pub type Socket = WebSocketStream<tokio_rustls::server::TlsStream<TcpStream>>;

/// The directory a hub keeps everything in when none is given.
pub fn default_dir(fastctx_root: &Path) -> PathBuf {
    fastctx_root.join("hub")
}

/// Creates a hub in `dir`: its World, its certificate and its founding
/// secret. Returns the founding code for the first machine.
pub fn init(dir: &Path, public: Vec<SocketAddr>, listen: SocketAddr) -> anyhow::Result<Code> {
    if Store::hub_file(dir).exists() {
        anyhow::bail!("{} already holds a hub", dir.display());
    }
    let certificate = HubCertificate::generate().map_err(anyhow::Error::msg)?;
    let hub = HubFile {
        world: WorldId::generate()?,
        public,
        listen,
        founding: Some(Secret::generate()?),
        founded_with: None,
    };
    Store::create(dir, &hub, &certificate)?;
    founding_code(dir)
}

/// The code the first machine founds the World with, while it is unused.
pub fn founding_code(dir: &Path) -> anyhow::Result<Code> {
    let store = Store::open(dir)?;
    let certificate = std::fs::read(Store::certificate_file(dir))?;
    let secret = store.hub.founding.clone().ok_or_else(|| anyhow::anyhow!("the World was founded already"))?;
    Ok(Code::Founding {
        hub: store.hub.public.clone(),
        cert: CertPin::of(&certificate),
        world: store.hub.world.clone(),
        secret,
    })
}

/// What every connection shares.
pub(crate) struct Hub {
    store: Mutex<Store>,
    sessions: Mutex<HashMap<MachineName, Session>>,
    /// The relay sessions, by relay.
    relays: Mutex<HashMap<MachineName, attached::RelaySession>>,
    calls: Mutex<HashMap<u64, control::Pending>>,
    waiting: Mutex<HashMap<u64, data::Waiting>>,
    /// Taken after `store` when both are held.
    ledgers: Mutex<ledger::Ledgers>,
    next: AtomicU64,
    log: log::Log,
}

/// A member's open control connection.
pub(crate) struct Session {
    generation: u64,
    out: mpsc::UnboundedSender<Out>,
}

pub(crate) enum Out {
    Frame(Down),
    /// Says why, then closes.
    End(Refusal),
}

impl Hub {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().expect("never poisoned")
    }

    fn sessions(&self) -> MutexGuard<'_, HashMap<MachineName, Session>> {
        self.sessions.lock().expect("never poisoned")
    }

    fn relays(&self) -> MutexGuard<'_, HashMap<MachineName, attached::RelaySession>> {
        self.relays.lock().expect("never poisoned")
    }

    fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Sends `frame` to `name` if it is connected.
    fn tell(&self, name: &MachineName, frame: Down) -> bool {
        match self.sessions().get(name) {
            Some(session) => session.out.send(Out::Frame(frame)).is_ok(),
            None => false,
        }
    }

    /// The member `key` belongs to, checking its proof of holding the key
    /// on this connection.
    fn member(&self, key: &PublicKey, proof: &Signature, binding: &[u8; 32]) -> Result<MachineName, Refusal> {
        if key.verify(Domain::Session, binding, proof).is_err() {
            return Err(Refusal::Broken { reason: "the proof of the machine's key does not verify".to_string() });
        }
        let store = self.store();
        if let Some(member) = store.table().by_key(key) {
            return Ok(member.name.clone());
        }
        let revoked = store.members.records.iter().find_map(|admitted| {
            let record = admitted.record.claim::<fastctx_net::trust::Record>().ok()?;
            (record.key == *key).then(|| store.table().revoked(&record.name).cloned()).flatten()
        });
        Err(match revoked {
            Some(revocation) => Refusal::Revoked { by: revocation.by, at: revocation.at, reason: revocation.reason },
            None => Refusal::NotMember,
        })
    }
}

/// Runs the hub kept in `dir` until the process is stopped.
pub async fn serve(dir: &Path) -> anyhow::Result<()> {
    let store = Store::open(dir)?;
    let certificate = HubCertificate {
        certificate: std::fs::read(Store::certificate_file(dir))?,
        key: std::fs::read(Store::key_file(dir))?,
    };
    let tls = tokio_rustls::TlsAcceptor::from(certificate.server_config().map_err(anyhow::Error::msg)?);
    let log = log::Log::open(&dir.join("hub.log"))?;
    let ledgers = ledger::Ledgers::open(dir)?;
    let listen = store.hub.listen;
    let listener =
        TcpListener::bind(listen).await.map_err(|error| anyhow::anyhow!("listening on {listen}: {error}"))?;
    log.line(format_args!(
        "hub {} for the World {} listening on {listen}; certificate {}",
        env!("CARGO_PKG_VERSION"),
        store.hub.world,
        certificate.pin()
    ));
    if store.hub.founding.is_some() {
        log.line(format_args!("the World is not founded yet; `fastctx hub code` prints the founding code"));
    }
    if !ledgers.older.is_empty() {
        log.line(format_args!(
            "{} plaintext ledgers of the network's second version wait for a machine with management rights to seal them",
            ledgers.older.len()
        ));
    }
    let hub = Arc::new(Hub {
        store: Mutex::new(store),
        sessions: Mutex::new(HashMap::new()),
        relays: Mutex::new(HashMap::new()),
        calls: Mutex::new(HashMap::new()),
        waiting: Mutex::new(HashMap::new()),
        ledgers: Mutex::new(ledgers),
        next: AtomicU64::new(0),
        log,
    });
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((tcp, peer)) => {
                    fastctx_net::dial::configure(&tcp);
                    tokio::spawn(connection(hub.clone(), tls.clone(), tcp, peer));
                }
                // Out of descriptors, for one: the next accept may succeed.
                Err(error) => {
                    hub.log.line(format_args!("accepting: {error}"));
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            },
            () = shutdown_signal() => break,
        }
    }
    hub.log.line(format_args!("shutting down"));
    let mut sessions: Vec<mpsc::UnboundedSender<Out>> =
        hub.sessions().values().map(|session| session.out.clone()).collect();
    sessions.extend(hub.relays().values().map(|relay| relay.out.clone()));
    for out in &sessions {
        let _ = out.send(Out::End(Refusal::ShuttingDown));
    }
    // Each writer says why and closes; one that cannot is gone anyway.
    let deadline = tokio::time::Instant::now() + SILENCE;
    while !(hub.sessions().is_empty() && hub.relays().is_empty()) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("installing a SIGTERM handler");
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

/// One TCP connection, from TLS to the end of what it was opened for.
async fn connection(hub: Arc<Hub>, tls: tokio_rustls::TlsAcceptor, tcp: TcpStream, peer: SocketAddr) {
    // Whoever connects must say who it is within the time the hub gives
    // any silent peer; scanners and stalled clients hold nothing longer.
    let opened = tokio::time::timeout(SILENCE, open(&tls, tcp)).await;
    let (mut socket, path, binding, hello) = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(_)) | Err(_) => return,
    };
    let Up::Hello { protocol, version, intent } = hello else {
        refuse(&mut socket, Refusal::Broken { reason: "the first frame must be a hello".to_string() }).await;
        return;
    };
    if protocol != PROTOCOL {
        hub.log.line(format_args!("{peer}: refused fastctx {version}, which speaks protocol {protocol}"));
        refuse(&mut socket, Refusal::Protocol { hub: PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string() }).await;
        return;
    }
    match (path.as_str(), intent) {
        (wire::CONTROL_PATH, Intent::Session { key, proof, letters }) => {
            control::session(hub, socket, peer, binding, key, proof, letters).await;
        }
        (wire::CONTROL_PATH, Intent::Relay { key, proof }) => {
            attached::session(hub, socket, peer, binding, key, proof).await;
        }
        (wire::CONTROL_PATH, Intent::Probe) => {
            let _ = send(&mut socket, &Down::Ready).await;
            let _ = socket.close(None).await;
        }
        // Only the founder joins straight away, with the founding secret;
        // an invited machine first proves it holds the invitation's key.
        (wire::CONTROL_PATH, Intent::Join { record, founding: Some(founding) }) => {
            let outcome = join::join(&hub, &record, Some(&founding));
            joined(&hub, &mut socket, peer, outcome).await;
        }
        (wire::CONTROL_PATH, Intent::Invited { invite, proof }) => {
            let endorsement = match join::endorsement(&hub, &invite, &proof, &binding) {
                Ok(endorsement) => endorsement,
                Err(reason) => {
                    hub.log.line(format_args!("{peer}: a join was refused: {reason}"));
                    refuse(&mut socket, Refusal::NotAdmitted { reason }).await;
                    return;
                }
            };
            if send(&mut socket, &Down::Endorsement { endorsement }).await.is_err() {
                return;
            }
            let record = match tokio::time::timeout(SILENCE, recv(&mut socket)).await {
                Ok(Ok(Some(Up::Admit { record }))) => record,
                // The machine looked at the invitation and did not take it.
                Ok(Ok(None)) => return,
                _ => {
                    refuse(
                        &mut socket,
                        Refusal::Broken { reason: "an invited machine sends its record next".to_string() },
                    )
                    .await;
                    return;
                }
            };
            let outcome = join::join(&hub, &record, None).and_then(|joined| {
                let admitted = record
                    .claim::<fastctx_net::trust::Record>()
                    .ok()
                    .and_then(|record| match record.credential {
                        fastctx_net::trust::Credential::Invited { admission } => {
                            admission.claim::<fastctx_net::trust::Admission>().ok()
                        }
                        fastctx_net::trust::Credential::Founder => None,
                    })
                    .and_then(|admission| admission.endorsement.claim::<fastctx_net::trust::Endorsement>().ok());
                match admitted {
                    Some(endorsement) if endorsement.invite == invite => Ok(joined),
                    _ => Err("the record is admitted by another invitation than the one proved".to_string()),
                }
            });
            joined(&hub, &mut socket, peer, outcome).await;
        }
        (wire::DATA_PATH, Intent::Data { key, proof, ticket, end, on_behalf }) => {
            data::end(hub, socket, binding, key, proof, ticket, end, on_behalf).await;
        }
        _ => {
            refuse(&mut socket, Refusal::Broken { reason: format!("that is not said on {path}") }).await;
        }
    }
}

/// Tells a joining machine whether it joined, and every member that the
/// members changed.
async fn joined(hub: &Hub, socket: &mut Socket, peer: SocketAddr, outcome: Result<join::Joined, String>) {
    match outcome {
        Ok(join::Joined { world, name, new }) => {
            if new {
                hub.log.line(format_args!("{peer}: {name} joined"));
            } else {
                hub.log.line(format_args!("{peer}: {name} joined again, as it was"));
            }
            let _ = send(socket, &Down::Joined { world, name }).await;
            let _ = socket.close(None).await;
            if new {
                for session in hub.sessions().values() {
                    let _ = session.out.send(Out::Frame(Down::MembersChanged));
                }
            }
        }
        Err(reason) => {
            hub.log.line(format_args!("{peer}: a join was refused: {reason}"));
            refuse(socket, Refusal::NotAdmitted { reason }).await;
        }
    }
}

type Opened = (Socket, String, [u8; 32], Up);

async fn open(tls: &tokio_rustls::TlsAcceptor, tcp: TcpStream) -> Result<Opened, String> {
    let stream = tls.accept(tcp).await.map_err(|error| error.to_string())?;
    let mut binding = [0_u8; 32];
    stream
        .get_ref()
        .1
        .export_keying_material(&mut binding, wire::EXPORTER_LABEL, None)
        .map_err(|error| error.to_string())?;
    let mut path = String::new();
    // The signature is the WebSocket library's, error response and all.
    #[allow(clippy::result_large_err)]
    let callback = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        path = request.uri().path().to_string();
        if path == wire::CONTROL_PATH || path == wire::DATA_PATH {
            Ok(response)
        } else {
            let mut refusal = ErrorResponse::new(None);
            *refusal.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::NOT_FOUND;
            Err(refusal)
        }
    };
    // The data path carries raw chunks; the control path only frames.
    let limit = wire::MAX_DATA_MESSAGE + wire::MAX_CONTROL_FRAME;
    let mut socket =
        tokio_tungstenite::accept_hdr_async_with_config(stream, callback, Some(fastctx_net::dial::config(limit)))
            .await
            .map_err(|error| error.to_string())?;
    let hello = match recv(&mut socket).await? {
        Some(frame) => frame,
        None => return Err("closed before saying hello".to_string()),
    };
    Ok((socket, path, binding, hello))
}

pub(crate) async fn send(socket: &mut Socket, frame: &Down) -> Result<(), String> {
    socket.send(WsMessage::Binary(wire::encode(frame).into())).await.map_err(|error| error.to_string())
}

pub(crate) async fn refuse(socket: &mut Socket, reason: Refusal) {
    let _ = send(socket, &Down::Refused { reason }).await;
    let _ = socket.close(None).await;
}

/// The next frame from a machine; `None` when it closed the connection.
pub(crate) async fn recv(socket: &mut Socket) -> Result<Option<Up>, String> {
    loop {
        match socket.next().await {
            None | Some(Ok(WsMessage::Close(_))) => return Ok(None),
            Some(Ok(WsMessage::Binary(bytes))) => {
                if bytes.len() > wire::MAX_CONTROL_FRAME {
                    return Err(format!("a frame of {} bytes, more than a control frame holds", bytes.len()));
                }
                return serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|error| format!("an unreadable frame: {error}"));
            }
            Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
            Some(Ok(WsMessage::Text(_))) => return Err("text instead of a frame".to_string()),
            Some(Err(error)) => return Err(error.to_string()),
        }
    }
}

/// Reports an I/O failure of the hub's own storage, which the hub cannot
/// work around: the member is told the connection broke, and sends again.
pub(crate) fn storage_failure(hub: &Hub, what: &str, error: &io::Error) -> Refusal {
    hub.log.line(format_args!("storage failed {what}: {error}"));
    Refusal::Broken { reason: format!("the hub could not store what it was given ({what}: {error})") }
}
