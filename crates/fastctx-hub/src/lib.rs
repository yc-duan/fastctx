//! The hub: the one machine every computer and phone of its accounts
//! connects to (`world-network.md`).
//!
//! Only the hub listens; every member dials out to it and keeps one control
//! connection open, so machines behind home routers, campus networks and
//! company firewalls need no open port. The hub is where who is who is
//! decided: accounts and their passwords, the devices logged in to them,
//! the machines registered under them, the machines shared and the agents'
//! logins lent between accounts, the machines acting for them, and projects. It
//! passes calls between connected members and keeps letters for those that
//! are away, only when the directory allows what is said; pairs the two
//! ends of each data transfer; and keeps the projects' ledgers. It sees
//! what passes through it, as MeshCentral's server does, and is to be
//! guarded as the most important machine there is.
//!
//! A relay (`relay`) only forwards the hub's port: the TLS between a
//! machine and the hub passes through it whole.

mod control;
mod data;
mod ledger;
mod log;
mod login;
mod machine_log;
mod manage;
pub mod relay;
mod releases;
mod store;
pub mod third;
mod upgrade;
mod views;

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use fastctx_net::directory::{Account, View};
use fastctx_net::ids::CertPin;
use fastctx_net::login::HubSecrets;
use fastctx_net::sign::{Bytes, Domain, PublicKey, Signature};
use fastctx_net::tls::HubCertificate;
use fastctx_net::wire::{self, Down, Intent, PROTOCOL, Refusal, SILENCE, Up};
use fastctx_types::{AccountName, HubName, MachineName};
use futures_util::{SinkExt as _, StreamExt as _};
use jiff::Timestamp;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

pub use store::HubFile;
use store::{Book, Store};

pub type Socket = WebSocketStream<tokio_rustls::server::TlsStream<TcpStream>>;

/// The directory a hub keeps everything in when none is given.
pub fn default_dir(fastctx_root: &Path) -> PathBuf {
    fastctx_root.join("hub")
}

/// What a new hub is made with.
pub struct New {
    pub name: HubName,
    /// Where machines reach the hub.
    pub public: Vec<SocketAddr>,
    pub listen: SocketAddr,
    /// The first account, its super administrator, and its password.
    pub admin: AccountName,
    pub password: String,
}

/// Creates a hub in `dir`: its id, certificate and login secrets, and its
/// first account, its super administrator, with its password. Returns the
/// certificate's pin, which devices learn when they first log in.
pub fn init(dir: &Path, new: New) -> anyhow::Result<CertPin> {
    if Store::hub_file(dir).exists() {
        anyhow::bail!("{} already holds a hub", dir.display());
    }
    let certificate = HubCertificate::generate().map_err(anyhow::Error::msg)?;
    let secrets = HubSecrets::generate();
    let record = password_record(&secrets, &new.admin, &new.password)?;
    let mut book = Book::default();
    book.directory.accounts.push(Account {
        name: new.admin.clone(),
        level: fastctx_net::directory::Level::Super,
        locked: false,
        opened: Timestamp::now(),
        collab: None,
    });
    book.passwords.insert(new.admin.clone(), Bytes(record));
    book.machine_log.push(
        &book.directory,
        &[],
        fastctx_types::machine_log::Actor { account: None, authenticated: false, member: None, peer: None },
        fastctx_types::machine_log::Action::OpenAccount { name: new.admin },
        fastctx_types::machine_log::Outcome::Done,
    )?;
    let hub = HubFile {
        id: fastctx_net::ids::new_hub_id()?,
        name: new.name,
        public: new.public,
        listen: new.listen,
        self_registration: false,
    };
    Store::create(dir, &hub, &certificate, &secrets.to_bytes(), &book)?;
    Ok(certificate.pin())
}

/// Sets the password of `account` on the hub kept in `dir`, from the hub's
/// own machine: how a super administrator who forgot theirs gets back in. The
/// hub must not be running.
pub fn reset_password(dir: &Path, account: &AccountName, password: &str) -> anyhow::Result<()> {
    let mut store = Store::open(dir)?;
    if store.book.directory.account(account).is_none() {
        anyhow::bail!("there is no account {account} on this hub");
    }
    let secrets = HubSecrets::from_bytes(&std::fs::read(Store::secrets_file(dir))?).map_err(anyhow::Error::msg)?;
    store.book.passwords.insert(account.clone(), Bytes(password_record(&secrets, account, password)?));
    let directory = store.book.directory.clone();
    store.book.machine_log.push(
        &directory,
        &[],
        fastctx_types::machine_log::Actor { account: None, authenticated: false, member: None, peer: None },
        fastctx_types::machine_log::Action::PasswordFinish { target: account.clone() },
        fastctx_types::machine_log::Outcome::Done,
    )?;
    store.save_book()?;
    Ok(())
}

/// What setting `account`'s password to `password` leaves with the hub: a
/// password is set as any is, both halves of OPAQUE here, and the hub keeps
/// only what it leaves.
fn password_record(secrets: &HubSecrets, account: &AccountName, password: &str) -> anyhow::Result<Vec<u8>> {
    fastctx_types::password::check(account.as_str(), password).map_err(anyhow::Error::msg)?;
    let (state, request) = fastctx_net::login::password_start(password).map_err(anyhow::Error::msg)?;
    let response = secrets.register_start(account, &request).map_err(anyhow::Error::msg)?;
    let upload = fastctx_net::login::password_finish(state, password, &response).map_err(anyhow::Error::msg)?;
    HubSecrets::register_finish(&upload).map_err(anyhow::Error::msg)
}

/// The hub's settings and the pin of its certificate.
pub fn describe(dir: &Path) -> anyhow::Result<(HubFile, CertPin)> {
    let store = Store::open(dir)?;
    let certificate = std::fs::read(Store::certificate_file(dir))?;
    Ok((store.hub, CertPin::of(&certificate)))
}

/// What every connection shares.
pub(crate) struct Hub {
    releases: Arc<releases::Releases>,
    store: Mutex<Store>,
    secrets: HubSecrets,
    /// The pin of the hub's certificate, written into tokens.
    pin: CertPin,
    sessions: Mutex<HashMap<MachineName, Session>>,
    calls: Mutex<HashMap<u64, control::Pending>>,
    data: Mutex<data::Pairing>,
    /// Taken after `store` when both are held.
    ledgers: Mutex<ledger::Ledgers>,
    limits: Mutex<login::Limits>,
    next: AtomicU64,
    log: log::Log,
    machine_log_changed: tokio::sync::Notify,
}

/// A member's open control connection.
pub(crate) struct Session {
    generation: u64,
    out: mpsc::UnboundedSender<Out>,
    since: Timestamp,
    /// The relay the member came through, as it said.
    through: Option<String>,
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
        if let Some(member) = store.book.directory.by_key(key) {
            return Ok(member.name.clone());
        }
        Err(match store.book.removed.iter().rev().find(|removed| removed.key == *key) {
            Some(removed) => {
                Refusal::Removed { by: removed.by.clone(), at: removed.at, reason: removed.reason.clone() }
            }
            None => Refusal::NotMember,
        })
    }

    /// `name`'s view of the directory now.
    fn view_for(&self, name: &MachineName) -> View {
        let presence: HashMap<MachineName, views::Presence> = self
            .sessions()
            .iter()
            .map(|(name, session)| {
                (name.clone(), views::Presence { since: Some(session.since), through: session.through.clone() })
            })
            .collect();
        views::view_for(&self.store(), &presence, name)
    }

    /// The directory changed, or who is online did: every member is told to
    /// look again, and what lost the standing it was allowed under ends.
    fn changed(&self) {
        self.machine_log_changed.notify_one();
        let outs: Vec<mpsc::UnboundedSender<Out>> =
            self.sessions().values().map(|session| session.out.clone()).collect();
        for out in outs {
            let _ = out.send(Out::Frame(Down::DirectoryChanged));
        }
        control::cut_calls(self);
        data::cut_streams(self);
    }
}

/// Runs the hub kept in `dir` until the process is stopped.
pub async fn serve(dir: &Path) -> anyhow::Result<()> {
    let store = Store::open(dir)?;
    let certificate = HubCertificate {
        certificate: std::fs::read(Store::certificate_file(dir))?,
        key: std::fs::read(Store::key_file(dir))?,
    };
    let secrets = HubSecrets::from_bytes(&std::fs::read(Store::secrets_file(dir))?).map_err(anyhow::Error::msg)?;
    let tls = tokio_rustls::TlsAcceptor::from(certificate.server_config().map_err(anyhow::Error::msg)?);
    let log = log::Log::open(&dir.join("hub.log"))?;
    let ledgers = ledger::Ledgers::open(dir)?;
    let listen = store.hub.listen;
    let listener =
        TcpListener::bind(listen).await.map_err(|error| anyhow::anyhow!("listening on {listen}: {error}"))?;
    log.line(format_args!(
        "hub {} ({}) {} listening on {listen}; certificate {}",
        store.hub.name,
        store.hub.id,
        env!("CARGO_PKG_VERSION"),
        certificate.pin()
    ));
    for line in &store.upgraded {
        log.line(format_args!("took up the directory of an earlier fastctx: {line}"));
    }
    let hub = Arc::new(Hub {
        releases: Arc::new(releases::Releases::new(dir, certificate.certificate.clone())?),
        store: Mutex::new(store),
        secrets,
        pin: certificate.pin(),
        sessions: Mutex::new(HashMap::new()),
        calls: Mutex::new(HashMap::new()),
        data: Mutex::new(data::Pairing::default()),
        ledgers: Mutex::new(ledgers),
        limits: Mutex::new(login::Limits::default()),
        next: AtomicU64::new(0),
        log,
        machine_log_changed: tokio::sync::Notify::new(),
    });
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((tcp, peer)) => {
                    fastctx_net::dial::configure(&tcp);
                    tokio::spawn(releases::http::serve(hub.clone(), tls.clone(), tcp, peer));
                }
                // Out of descriptors, for one: the next accept may succeed.
                Err(error) => {
                    hub.log.line(format_args!("accepting: {error}"));
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            },
            () = shutdown_signal() => break,
            expired = machine_log::expire(&hub) => expired?,
        }
    }
    hub.log.line(format_args!("shutting down"));
    let sessions: Vec<mpsc::UnboundedSender<Out>> =
        hub.sessions().values().map(|session| session.out.clone()).collect();
    for out in &sessions {
        let _ = out.send(Out::End(Refusal::ShuttingDown));
    }
    // Each writer says why and closes; one that cannot is gone anyway.
    let deadline = tokio::time::Instant::now() + SILENCE;
    while !hub.sessions().is_empty() && tokio::time::Instant::now() < deadline {
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

pub(crate) async fn connected(
    hub: Arc<Hub>,
    mut socket: Socket,
    path: String,
    binding: [u8; 32],
    hello: Up,
    peer: SocketAddr,
) {
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
        (wire::CONTROL_PATH, Intent::Session { key, proof, letters, through }) => {
            control::session(hub, socket, peer, binding, key, proof, letters, through).await;
        }
        (wire::CONTROL_PATH, Intent::Login { account, request }) => {
            login::with_password(hub, socket, peer, binding, account, request).await;
        }
        (wire::CONTROL_PATH, Intent::Token { token, proof, member }) => {
            login::with_token(hub, socket, peer, binding, token, proof, member).await;
        }
        (wire::CONTROL_PATH, Intent::Register { account, request }) => {
            login::open_account(hub, socket, peer, account, request).await;
        }
        (wire::CONTROL_PATH, Intent::Probe) => {
            let _ = send(&mut socket, &Down::Ready).await;
            let _ = socket.close(None).await;
        }
        (wire::DATA_PATH, Intent::Data { key, proof, stream, end }) => {
            data::end(hub, socket, binding, key, proof, stream, end).await;
        }
        _ => {
            refuse(&mut socket, Refusal::Broken { reason: format!("that is not said on {path}") }).await;
        }
    }
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
