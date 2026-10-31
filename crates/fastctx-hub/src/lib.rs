//! The hub: the one machine every computer and phone of its accounts
//! connects to (`world-network.md`).
//!
//! Only the hub listens; every member dials out to it and keeps one control
//! connection open, so machines behind home routers, campus networks and
//! company firewalls need no open port. The hub is where who is who is
//! decided: accounts and their passwords, the devices logged in to them,
//! the machines registered under them, the machines shared and the agents'
//! logins assisting other accounts, the machines acting for them, and projects. It
//! passes calls between connected members and keeps letters for those that
//! are away, only when the directory allows what is said; pairs the two
//! ends of each data transfer; and keeps the projects' ledgers. It sees
//! what passes through it, as MeshCentral's server does, and is to be
//! guarded as the most important machine there is.
//!
//! A machine the super administrator allowed direct connections to takes
//! them itself (`direct`): what passes between it and the machines linked
//! with it does not come here, and a machine whose own line is down reaches
//! the hub through it, the TLS between that machine and the hub passing
//! through it whole.
//!
//! The hub can move to another machine with all it keeps (`moving`): the
//! directory it runs from then stands by for the move, serves, or, at the
//! old place, forwards to the new one, as its files say.

mod account_records;
mod afoot;
mod assists;
mod catalog;
mod control;
mod data;
mod direct;
pub mod dissolve;
mod expiry;
mod forget;
mod health;
mod history_projection;
mod hold_back;
mod hub_machine;
mod ledger;
mod locks;
mod log;
mod login;
mod machine_log;
mod mailbox;
mod manage;
mod measure;
mod moving;
mod network;
mod origin;
mod outq;
mod owned_records;
mod plan_calls;
mod plan_copies;
mod post;
mod presence;
mod project_deletions;
mod project_entry;
mod question_parts;
mod record_subscriptions;
mod records;
mod releases;
mod renewal;
mod sentinel_home;
mod skill_files;
mod skill_storage;
mod store;
pub mod third;
mod updates;
mod upgrade;
mod views;
mod watchdog;
mod worked;

use crate::locks::{Guard as MutexGuard, Mutex};
use fastctx_platform::doing::Rank;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

pub use moving::standby::prepare as prepare_arrival;
pub use store::HubFile;
use store::{Book, Store};

pub type Socket = WebSocketStream<tokio_rustls::server::TlsStream<fastctx_net::pulse::Pulsing<TcpStream>>>;

/// When the bytes of `socket` last moved, each way (`fastctx_net::pulse`).
pub(crate) fn pulse_of(socket: &Socket) -> std::sync::Arc<fastctx_net::pulse::Pulse> {
    socket.get_ref().get_ref().0.pulse()
}

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
    if let Some((name, untold)) = dissolve::dissolved(dir)? {
        anyhow::bail!(
            "{} holds the hub of the dissolved network {name}, still telling {untold} machines that were away; `fastctx hub retire` ends it",
            dir.display()
        );
    }
    if Store::hub_file(dir).exists() || dir.join(store::shelf::FILE).exists() {
        anyhow::bail!("{} already holds a hub", dir.display());
    }
    // A hub that moved away left only its log and a note here.
    if moving::retired(dir)? {
        std::fs::remove_file(dir.join(moving::RETIRED))?;
    }
    let certificate = HubCertificate::generate().map_err(anyhow::Error::msg)?;
    let secrets = HubSecrets::generate();
    let record = password_record(&secrets, &new.admin, &new.password)?;
    let mut book = Book::default();
    book.directory.accounts.push(Arc::new(Account {
        name: new.admin.clone(),
        level: fastctx_net::directory::Level::Super,
        locked: false,
        opened: Timestamp::now(),
        collab: None,
        profile: None,
        promotion: false,
    }));
    Arc::make_mut(&mut book.passwords).insert(new.admin.clone(), Bytes(record));
    book.machine_log.push(
        &book.directory,
        &[],
        fastctx_types::machine_log::Actor { account: None, authenticated: false, member: None, peer: None },
        fastctx_types::machine_log::Action::OpenAccount { name: new.admin, visitor: false },
        fastctx_types::machine_log::Outcome::Done,
    )?;
    let hub = HubFile {
        id: fastctx_net::ids::new_hub_id()?,
        name: new.name,
        public: new.public,
        listen: new.listen,
        self_registration: false,
        dissolved: None,
        updates: Default::default(),
        due: Vec::new(),
        repairs: Vec::new(),
        package_targets: Default::default(),
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
    Arc::make_mut(&mut store.book.passwords)
        .insert(account.clone(), Bytes(password_record(&secrets, account, password)?));
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
    let hub = Store::read_hub(dir)?;
    let certificate = std::fs::read(Store::certificate_file(dir))?;
    Ok((hub, CertPin::of(&certificate)))
}

/// What the hub needs of the program that runs it when it leaves a
/// machine: a hub that moved away, once its old data is deleted, has its
/// system service removed, and says what came of that.
pub struct Leave {
    pub remove_service: Box<RemoveService>,
}

/// Removes the system service of the hub kept in the directory given, and
/// says what came of that.
pub type RemoveService = dyn Fn(&Path) -> Result<String, String> + Send + Sync;

/// The id of the hub and of the move arriving in `dir`, while one stands by
/// there to take over.
pub fn arriving(dir: &Path) -> io::Result<Option<(fastctx_net::ids::HubId, String)>> {
    let Some(arriving) = moving::standby::arriving_in(dir)? else { return Ok(None) };
    let hub: HubFile = store::read(&crate::store::Store::hub_file(dir))?;
    Ok(Some((hub.id, arriving.id)))
}

/// What every connection shares.
pub(crate) struct Hub {
    releases: Arc<releases::Releases>,
    catalog: catalog::Catalog,
    post: Arc<post::Post>,
    this: std::sync::OnceLock<std::sync::Weak<Hub>>,
    network: tokio::runtime::Handle,
    secrets: HubSecrets,
    /// The pin of the hub's certificate, written into tokens.
    pin: CertPin,
    sessions: Mutex<HashMap<MachineName, Session>>,
    /// The view last given each session that reads what changes in it,
    /// which the next change is made against. Taken after `store` and
    /// `sessions` when held with them, and nothing taken while it is.
    given: Mutex<HashMap<MachineName, Given>>,
    /// The checks of machines' direct addresses under way (`direct`).
    trials: Mutex<direct::Trials>,
    calls: Mutex<control::InFlight>,
    data: Mutex<data::Pairing>,
    /// Taken after `store` when both are held.
    ledgers: Mutex<ledger::Ledgers>,
    limits: Mutex<login::Limits>,
    next: AtomicU64,
    log: Arc<log::Log>,
    machine_log_changed: tokio::sync::Notify,
    moving: moving::Moving,
    /// Told when the directory changes, so the assistance's deadlines are
    /// read again.
    assists_changed: tokio::sync::Notify,
    next_assist_end: assists::NextEnd,
    /// What is afoot on each machine, taken after `sessions` and `store`.
    afoots: afoot::Afoots,
    /// The network's versions and the packages kept here, taken after
    /// `sessions` and `store`.
    updates: Mutex<updates::Updates>,
    /// Told when the network is dissolved, and as each machine on it is
    /// told so (`dissolve`).
    dissolution: tokio::sync::Notify,
    /// Told when every member is to look at the directory again
    /// (`tell_directory`).
    directory_changed: Arc<tokio::sync::Notify>,
    /// Told when who is online changed, for the members whose sessions do
    /// not read that as it changes (`tell_directory`).
    presence_changed: Arc<tokio::sync::Notify>,
    /// How many times who is online changed (`Hub::present`).
    presence: AtomicU64,
    /// Held while a change in who is online is counted, looked at and
    /// queued, so that of two at once the later counted is queued later.
    pending_presence: Mutex<HashMap<MachineName, u64>>,
    presence_pending: Arc<tokio::sync::Notify>,
    /// The directory's index for telling who is online (`Hub::index_of`).
    /// What the hub counts for its health (`health`).
    health: health::Health,
    watch: watchdog::Health,
    diagnostic_dir: PathBuf,
    account_records: account_records::Records,
}

/// The view a session that reads what changes in its view was last given
/// (`fastctx_net::view_delta`), numbered among those it was given.
#[derive(Clone)]
pub(crate) struct Given {
    generation: u64,
    view: Arc<View>,
    /// When it read the directory (`store::Store::view_reads`).
    read: (u64, u64),
}

/// The most bytes a change to a view takes in one frame; a larger one is
/// told as `Down::DirectoryChanged`, and the member asks for the view whole.
const VIEW_CHANGE_AT_MOST: usize = 192 * 1024;

/// A member's open control connection.
pub(crate) struct Session {
    replicated_records: bool,
    key: PublicKey,
    account_records: bool,
    record_keys: Option<record_subscriptions::Records>,
    skill_files: bool,
    conversation_plans: bool,
    scratch_locations: bool,
    call_details: bool,
    generation: u64,
    out: outq::Outq,
    since: Timestamp,
    /// Where the connection comes from: the member, or the machine that
    /// forwards it here.
    peer: SocketAddr,
    /// The machine the member came through, as it said.
    through: Option<String>,
    /// Whether the member can take direct connections, as it said.
    direct: bool,
    /// The addresses of its network cards, as it said.
    interfaces: Vec<std::net::IpAddr>,
    /// How its lines go, as it last said.
    paths: Option<fastctx_net::directory::Paths>,
    /// Whether it reads what is afoot on other machines.
    afoot: bool,
    /// Whether it deletes what it keeps of a conversation when told.
    forgets: bool,
    /// Whether it reads the network's versions.
    versions: bool,
    version_parts: bool,
    /// Whether it reads that a call it answers is over for its caller.
    cancels: bool,
    /// Whether it reads who is online as it changes (`Down::Presence`).
    presence: bool,
    project_entry: bool,
    /// Whether it reads what changed in its view (`Down::View`).
    view_deltas: bool,
}

pub(crate) enum Out {
    Frame(Down),
    /// Says why, then closes.
    End(Refusal),
}

impl Hub {
    async fn blocking<T: Send + 'static>(&self, work: impl FnOnce(Arc<Hub>) -> T + Send + 'static) -> T {
        let hub = self.this.get().and_then(std::sync::Weak::upgrade).expect("hub running");
        match tokio::task::spawn_blocking(move || work(hub)).await {
            Ok(value) => value,
            Err(error) => std::panic::resume_unwind(error.into_panic()),
        }
    }

    async fn changed_async(&self) {
        self.blocking(|hub| hub.changed()).await;
    }

    /// Waits for catalog edits admitted before this barrier and their database
    /// writes. Freeze callers stop admission before awaiting this boundary.
    async fn flush_catalog(&self) -> io::Result<()> {
        self.blocking(|hub| drop(hub.edit_store())).await;
        self.store().shelf.0.write(0, |_| Ok(())).await.map_err(store::shelf::other)
    }

    fn store(&self) -> Arc<store::Snapshot> {
        self.catalog.read()
    }

    fn edit_store(&self) -> catalog::Editing {
        self.catalog.edit()
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
        if let Some(member) = store.indexed().by_key(key) {
            return Ok(member.name.clone());
        }
        Err(match store.book.removed.iter().rev().find(|removed| removed.key == *key) {
            Some(removed) => Refusal::Removed {
                by: removed.by.clone(),
                at: removed.at,
                reason: removed.reason.clone(),
                dissolved: removed.dissolved,
                then: removed.then,
            },
            None => Refusal::NotMember,
        })
    }

    /// `name`'s view of the directory now.
    fn view_for(&self, name: &MachineName, key: PublicKey, generation: u64) -> Result<View, String> {
        // Counted before who is online is read: a change after that is told
        // past the view, and told again if the view holds it already.
        let rev = self.presence.load(Ordering::SeqCst);
        let trial = self.trial_of(name);
        let (mut view, read) = {
            let store = self.store();
            let read = (store.sequence, store.view_reads.fetch_add(1, Ordering::SeqCst) + 1);
            if store.indexed().member(name).is_none_or(|member| member.key != key) {
                return Err("the member was replaced".into());
            }
            (views::view_for(self, &store, self, name, trial)?, read)
        };
        view.presence = rev;
        self.give(name, generation, &mut view, read);
        Ok(view)
    }

    /// Numbers `view`, given whole to `name`, and keeps it as what the next
    /// change to it is made against, when `name`'s session reads changes.
    /// One read before what the session was last given is given that
    /// instead: a view made earlier is never numbered past a later one.
    fn give(&self, name: &MachineName, generation: u64, view: &mut View, read: (u64, u64)) {
        let mut copy = view.clone();
        let sessions = self.sessions();
        if !sessions.get(name).is_some_and(|session| session.view_deltas && session.generation == generation) {
            return;
        }
        let mut given = self.given.lock().expect("never poisoned");
        let before = given.get(name).filter(|given| given.generation == generation);
        if let Some(later) = before.filter(|given| given.read > read).map(|given| given.view.clone()) {
            drop(given);
            drop(sessions);
            *view = View::clone(&later);
            return;
        }
        view.revision = before.map_or(1, |given| given.view.revision + 1);
        copy.revision = view.revision;
        let before = given.insert(name.clone(), Given { generation, view: Arc::new(copy), read });
        drop(given);
        drop(sessions);
        drop(before);
    }

    /// `name`'s session `generation` ended: what it was given goes with it.
    pub(crate) fn forget_view(&self, name: &MachineName, generation: u64) {
        let mut given = self.given.lock().expect("never poisoned");
        let before =
            if given.get(name).is_some_and(|given| given.generation == generation) { given.remove(name) } else { None };
        drop(given);
        drop(before);
    }

    /// The directory changed: each session that reads what changes in its
    /// view is sent what changed in it, all of them made from one index;
    /// one whose view did not change is sent nothing, and one never given
    /// a whole view, or whose change is too large for a frame, is told to
    /// ask for it.
    fn push_views(&self, sessions: Vec<(MachineName, PublicKey, u64, outq::Outq)>) {
        let trials: Vec<_> = sessions.iter().map(|(name, _, _, _)| self.trial_of(name)).collect();
        let (views, read): (Vec<Result<View, String>>, (u64, u64)) = {
            let store = self.store();
            let read = (store.sequence, store.view_reads.fetch_add(1, Ordering::SeqCst) + 1);
            let indexed = store.indexed();
            let views = sessions
                .iter()
                .zip(trials)
                .map(|((name, key, _, _), trial)| {
                    if indexed.member(name).is_none_or(|member| member.key != *key) {
                        return Err("the member was replaced".into());
                    }
                    views::view_for_in(self, &store, &indexed, self, name, trial)
                })
                .collect();
            (views, read)
        };
        for ((name, _, generation, out), view) in sessions.into_iter().zip(views) {
            let mut view = match view {
                Ok(view) => view,
                Err(error) => {
                    self.log.line(format_args!("reading {name}'s machine notices: {error}"));
                    let _ = out.send(Out::Frame(Down::DirectoryChanged));
                    continue;
                }
            };
            let before = self
                .given
                .lock()
                .expect("never poisoned")
                .get(&name)
                .filter(|given| given.generation == generation)
                .cloned();
            let Some(before) = before else {
                let _ = out.send(Out::Frame(Down::DirectoryChanged));
                continue;
            };
            // What it was given since was read after this: it holds this.
            if before.read > read {
                continue;
            }
            // A change counts no change of who is online (`view_delta`'s
            // `PRESENCE`): those are told apart, and one told before it
            // may reach the member after it, so it holds them only as far
            // as the view it starts from did.
            view.presence = before.view.presence;
            let base = before.view.revision;
            view.revision = base + 1;
            let delta = fastctx_net::view_delta::between(&before.view, &view, base, base + 1);
            let frame = match delta {
                Ok(None) => {
                    let mut given = self.given.lock().expect("never poisoned");
                    if let Some(current) = given
                        .get_mut(&name)
                        .filter(|current| current.generation == generation && current.read == before.read)
                    {
                        current.read = read;
                    }
                    continue;
                }
                Ok(Some(delta)) => Down::View { delta: Box::new(delta) },
                Err(error) => {
                    self.log.line(format_args!("making what changed in {name}'s view: {error}"));
                    let _ = out.send(Out::Frame(Down::DirectoryChanged));
                    continue;
                }
            };
            if serde_json::to_vec(&frame).map_or(true, |bytes| bytes.len() > VIEW_CHANGE_AT_MOST) {
                let _ = out.send(Out::Frame(Down::DirectoryChanged));
                continue;
            }
            {
                let mut given = self.given.lock().expect("never poisoned");
                if !given.get(&name).is_some_and(|current| {
                    current.generation == generation && current.read == before.read && current.view.revision == base
                }) {
                    continue;
                }
                given.insert(name, Given { generation, view: Arc::new(view), read });
            }
            let _ = out.send(Out::Frame(frame));
        }
    }

    fn index_of(&self, store: &store::Snapshot) -> Arc<fastctx_net::directory_index::Index> {
        store.index.clone()
    }

    /// Resolves once `name`'s session ends, at once when it has none.
    async fn gone(&self, name: &MachineName) {
        let out = self.sessions().get(name).map(|session| session.out.clone());
        if let Some(out) = out {
            out.closed().await;
        }
    }

    /// Whether `name` is connected, or how, changed: a member whose session
    /// reads that is told how it is now, as it may see it; the others look
    /// at the whole directory again. Nothing of who may do what changed, so
    /// no call or transfer is cut; a session just come is told what is
    /// afoot.
    fn present(&self, name: &MachineName) {
        let _timing = self.health.presence_queue.time();
        let rev = self.presence.fetch_add(1, Ordering::SeqCst) + 1;
        self.pending_presence
            .lock()
            .expect("presence changes")
            .entry(name.clone())
            .and_modify(|pending| *pending = (*pending).max(rev))
            .or_insert(rev);
        self.presence_pending.notify_one();
    }

    fn publish_presence(&self, name: &MachineName, rev: u64) {
        let _timing = self.health.presence.time();
        let store = self.store();
        let indexed = store.indexed();
        for reader in store.audience.readers(&indexed, name) {
            if reader == *name {
                continue;
            }
            let Some(member) = indexed.member(&reader) else { continue };
            let session = self
                .sessions()
                .get(&reader)
                .filter(|session| session.key == member.key)
                .map(|session| (session.out.clone(), session.presence));
            let Some((out, reads)) = session else { continue };
            if !reads {
                self.presence_changed.notify_one();
                continue;
            }
            if let Some(presence) = views::presence_for(&store, &indexed, self, &reader, name) {
                let _ = out.send(Out::Frame(Down::Presence { machine: name.clone(), presence, rev }));
            }
        }
    }

    /// The directory changed, or who is online did: every member is told to
    /// look again, and what lost the standing it was allowed under ends.
    fn changed(&self) {
        afoot::directory_changed(self);
        self.account_records.settle(&self.store().book.directory);
        updates::directory_changed(self);
        self.machine_log_changed.notify_one();
        self.assists_changed.notify_one();
        self.directory_changed.notify_one();
        control::cut_calls(self);
        data::cut_streams(self);
        moving::follow::settle(self);
        forget::prune(self);
        afoot::settle(self);
    }
}

/// How long the hub waits after telling every member the directory changed
/// before it tells them again.
const DIRECTORY_SPACING: std::time::Duration = std::time::Duration::from_secs(1);

/// Tells every member to look at the directory again whenever it changed,
/// and the members whose sessions do not read who is online as it changes
/// whenever that did, at most once each `DIRECTORY_SPACING`: a machine that
/// keeps coming and going does not have every member read it all again each
/// time (`specs/world-concurrency.md`, 机器之间). What lost its standing is cut
/// at once, by `Hub::changed` itself.
async fn tell_directory(
    hub: std::sync::Weak<Hub>,
    changed: Arc<tokio::sync::Notify>,
    present: Arc<tokio::sync::Notify>,
) {
    loop {
        let every = tokio::select! {
            () = changed.notified() => true,
            () = present.notified() => false,
        };
        let Some(hub) = hub.upgrade() else { return };
        hub.blocking(move |hub| {
            let mut outs: Vec<outq::Outq> = Vec::new();
            let mut changes = Vec::new();
            for (name, session) in hub.sessions().iter() {
                if every && session.view_deltas {
                    changes.push((name.clone(), session.key, session.generation, session.out.clone()));
                } else if every || !session.presence {
                    outs.push(session.out.clone());
                }
            }
            for out in outs {
                let _ = out.send(Out::Frame(Down::DirectoryChanged));
            }
            if !changes.is_empty() {
                hub.push_views(changes);
            }
        })
        .await;
        drop(hub);
        tokio::time::sleep(DIRECTORY_SPACING).await;
    }
}

/// What a mode of the hub's directory ends with.
pub(crate) enum Next {
    /// The process is stopped.
    Stop,
    /// The directory changed what it runs as; the listener goes on to it.
    Again(Option<Kept>),
    /// The network was dissolved and every machine on it told: the hub
    /// retires, for the reason given.
    Retire(String),
}

/// A listener one mode leaves to the next, and where it listens.
pub(crate) struct Kept {
    pub listener: TcpListener,
    pub at: SocketAddr,
}

/// The listener on `listen`: the one kept, when it listens there.
pub(crate) async fn bind(kept: Option<Kept>, listen: SocketAddr) -> anyhow::Result<Kept> {
    match kept {
        Some(kept) if kept.at == listen => Ok(kept),
        _ => {
            let listener =
                TcpListener::bind(listen).await.map_err(|error| anyhow::anyhow!("listening on {listen}: {error}"))?;
            Ok(Kept { listener, at: listen })
        }
    }
}

/// Runs the hub kept in `dir` until the process is stopped: serving, or
/// standing by for a move here, or forwarding after a move away.
pub async fn serve(dir: &Path, leave: Leave) -> anyhow::Result<()> {
    let log = Arc::new(log::Log::open(&dir.join("hub.log"))?);
    let network = network::Network::new()?;
    let mut kept = None;
    loop {
        let next = match moving::mode(dir)? {
            moving::Mode::Serving => serving(dir, kept.take(), log.clone(), network.handle()).await?,
            moving::Mode::Standby(arriving) => {
                moving::standby::run(dir, kept.take(), arriving, log.clone(), &leave).await?
            }
            moving::Mode::Old(outgoing) => moving::old::run(dir, kept.take(), outgoing, log.clone(), &leave).await?,
            moving::Mode::Retired => {
                drop(kept.take());
                log.line(format_args!("no hub runs here: its data was deleted ({})", moving::RETIRED));
                shutdown_signal().await;
                Next::Stop
            }
        };
        match next {
            Next::Stop => return Ok(()),
            Next::Again(listener) => kept = listener,
            Next::Retire(why) => moving::retire(dir, &why, &log, &leave),
        }
    }
}

async fn serving(
    dir: &Path,
    kept: Option<Kept>,
    log: Arc<log::Log>,
    network: tokio::runtime::Handle,
) -> anyhow::Result<Next> {
    let mut store = Store::open(dir)?;
    // An assistance whose deadline passed while the hub was stopped is
    // ended before any member is heard.
    assists::end_due(&mut store, Timestamp::now())?;
    let certificate = HubCertificate {
        certificate: std::fs::read(Store::certificate_file(dir))?,
        key: std::fs::read(Store::key_file(dir))?,
    };
    let secrets = HubSecrets::from_bytes(&std::fs::read(Store::secrets_file(dir))?).map_err(anyhow::Error::msg)?;
    let tls = releases::http::Tls::of(&certificate).map_err(anyhow::Error::msg)?;
    let account_records = account_records::Records::open(dir, &store.book.directory)?;
    let mut ledgers = ledger::Ledgers::open(dir, &store.hub.id)?;
    let swept = ledgers.sweep(&store.book)?;
    let listen = store.hub.listen;
    let kept = bind(kept, listen).await?;
    log.line(format_args!(
        "hub {} ({}) {} listening on {listen}; certificate {}",
        store.hub.name,
        store.hub.id,
        env!("CARGO_PKG_VERSION"),
        certificate.pin()
    ));
    for town in &swept {
        log.line(format_args!("deleted the ledger of {town}, a project deleted before the hub stopped"));
    }
    for line in &store.upgraded {
        log.line(format_args!("took up the directory of an earlier fastctx: {line}"));
    }
    for line in &store.tidied {
        log.line(format_args!("on opening, {line}"));
    }
    let mut left = Vec::new();
    let updates = updates::Updates::new(dir, &store.shelf, &store.hub.due, &mut left).map_err(anyhow::Error::msg)?;
    for line in left {
        log.line(format_args!("on opening, {line}"));
    }
    // A move under way goes on; one called off is told to the new place.
    let driven = store.outgoing.as_ref().is_some_and(|outgoing| {
        outgoing.pending() || (outgoing.stage == fastctx_types::hub_move::MoveStage::Cancelled && outgoing.prepared)
    });
    let post = store.enable_runtime()?;
    let (switched, mut switch) = tokio::sync::oneshot::channel();
    let hub = Arc::new(Hub {
        releases: Arc::new(releases::Releases::new(dir, certificate.certificate.clone())?),
        catalog: catalog::Catalog::new(store)?,
        post,
        this: std::sync::OnceLock::new(),
        network,
        secrets,
        pin: certificate.pin(),
        sessions: Mutex::new("hub sessions", Rank::RECORDS, HashMap::new()),
        given: Mutex::new("hub given", Rank::WRITE_FACTS, HashMap::new()),
        trials: Mutex::new("hub trials", Rank::COPYING, direct::Trials::default()),
        calls: Mutex::new("hub calls", Rank::EMITTING, control::InFlight::default()),
        data: Mutex::new("hub data", Rank::WRITING, data::Pairing::default()),
        ledgers: Mutex::new("hub ledgers", Rank::WRITE_FACTS, ledgers),
        limits: Mutex::new("hub limits", Rank::DRAINING, login::Limits::default()),
        next: AtomicU64::new(0),
        log,
        machine_log_changed: tokio::sync::Notify::new(),
        moving: moving::Moving::new(switched),
        assists_changed: tokio::sync::Notify::new(),
        next_assist_end: assists::NextEnd::default(),
        afoots: afoot::Afoots::default(),
        updates: Mutex::new("hub updates", Rank::WRITE_FACTS, updates),
        dissolution: tokio::sync::Notify::new(),
        directory_changed: Arc::new(tokio::sync::Notify::new()),
        presence_changed: Arc::new(tokio::sync::Notify::new()),
        presence: AtomicU64::new(0),
        pending_presence: Mutex::new("hub presence", Rank::DRAINING, Default::default()),
        presence_pending: Arc::new(tokio::sync::Notify::new()),
        health: health::Health::default(),
        watch: watchdog::Health::default(),
        diagnostic_dir: dir.to_path_buf(),
        account_records,
    });
    let _ = hub.this.set(Arc::downgrade(&hub));
    hub.post.attach(Arc::downgrade(&hub));
    let _watchdog = watchdog::Watchdog::start(&hub)?;
    if let Err(error) = hub.blocking(|hub| hub_machine::settle(&hub)).await {
        hub.log.line(format_args!("revoking shares of the hub machine: {error}"));
    }
    tokio::spawn(health::keep(Arc::downgrade(&hub)));
    tokio::spawn(data::expire(Arc::downgrade(&hub)));
    tokio::spawn(expiry::settle(Arc::downgrade(&hub)));
    tokio::spawn(updates::look_every(Arc::downgrade(&hub)));
    tokio::spawn(presence::run(Arc::downgrade(&hub), hub.presence_pending.clone()));
    tokio::spawn(tell_directory(Arc::downgrade(&hub), hub.directory_changed.clone(), hub.presence_changed.clone()));
    updates::start(&hub);
    afoot::start(&hub);
    if driven {
        moving::out::drive(&hub);
    }
    let settled = dissolve::settled(&hub);
    tokio::pin!(settled);
    let replaced = renewal::replaced();
    tokio::pin!(replaced);
    // These workers enqueue durable work before awaiting it. Recreating them
    // on every accept would abandon their waits and duplicate that work.
    let expiring_log = machine_log::expire(&hub);
    let expiring_assists = assists::expire(&hub);
    let stopping = shutdown_signal();
    tokio::pin!(expiring_log, expiring_assists, stopping);
    let switched = loop {
        tokio::select! {
            accepted = network::accept(&hub.network, &kept.listener) => match accepted {
                Ok((tcp, peer)) => {
                    fastctx_net::dial::configure(&tcp);
                    let host = releases::http::Host::Hub(hub.clone());
                    tokio::spawn(releases::http::serve(host, tls.clone(), tcp, peer));
                }
                // Out of descriptors, for one: the next accept may succeed.
                Err(error) => {
                    hub.log.line(format_args!("accepting: {error}"));
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            },
            () = &mut stopping => break false,
            version = &mut replaced => {
                hub.log.line(format_args!("the program this hub runs from is {version} now: stopping for its service to start it on that"));
                break false;
            }
            expired = &mut expiring_log => expired?,
            ended = &mut expiring_assists => ended?,
            Ok(()) = &mut switch => break true,
            why = &mut settled => {
                hub.log.line(format_args!("{why}: the hub retires"));
                hub.moving.stop_taking();
                hub.moving.stop();
                let deadline = tokio::time::Instant::now() + SILENCE;
                if !hub.moving.settled(deadline).await {
                    anyhow::bail!("retirement did not finish: persistent work is still under way after {} s", SILENCE.as_secs());
                }
                hub.flush_catalog().await?;
                return Ok(Next::Retire(why));
            }
        }
    };
    if switched {
        // The hub moved away: every connection still open here ends, and the
        // old place forwards on the same port.
        hub.moving.stop();
        hub.flush_catalog().await?;
        hub.post.flush().await.map_err(anyhow::Error::msg)?;
        return Ok(Next::Again(Some(kept)));
    }
    hub.moving.stop_taking();
    hub.log.line(format_args!("shutting down"));
    let sessions: Vec<outq::Outq> = hub.sessions().values().map(|session| session.out.clone()).collect();
    for out in &sessions {
        let _ = out.send(Out::End(Refusal::ShuttingDown));
    }
    // Each writer says why and closes, and what the sessions began, as a
    // letter being written, is done; one that cannot is gone anyway.
    let deadline = tokio::time::Instant::now() + SILENCE;
    while !hub.sessions().is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    if !hub.moving.settled(deadline).await {
        anyhow::bail!("shutdown did not finish: persistent work is still under way after {} s", SILENCE.as_secs());
    }
    hub.flush_catalog().await?;
    hub.post.flush().await.map_err(anyhow::Error::msg)?;
    updates::flush(&hub).await.map_err(anyhow::Error::msg)?;
    Ok(Next::Stop)
}

pub(crate) async fn shutdown_signal() {
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
        hub.log.line(format_args!("{peer}: refused a connection on {path} whose first frame was not a hello"));
        refuse(&mut socket, Refusal::Broken { reason: "the first frame must be a hello".to_string() }).await;
        return;
    };
    if !wire::accepts(protocol) {
        hub.log.line(format_args!("{peer}: refused fastctx {version}, which speaks protocol {protocol}"));
        refuse(&mut socket, Refusal::Protocol { hub: PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string() }).await;
        return;
    }
    let stopped = hub.clone();
    tokio::select! {
        () = intended(hub, socket, path, binding, intent, peer) => {}
        // The hub moved away: what is still open here ends.
        () = stopped.moving.stopped() => {}
    }
}

async fn intended(
    hub: Arc<Hub>,
    mut socket: Socket,
    path: String,
    binding: [u8; 32],
    intent: Intent,
    peer: SocketAddr,
) {
    // A dissolved network lets nobody in again: a device asking to is told
    // so, a member's own session by its removal (`Hub::member`).
    let dissolved = {
        let store = hub.store();
        store.hub.dissolved.is_some().then(|| store.hub.name.clone())
    };
    if let Some(name) = dissolved
        && matches!(intent, Intent::Login { .. } | Intent::Token { .. } | Intent::Peek { .. } | Intent::Register { .. })
    {
        refuse(&mut socket, Refusal::Login { reason: format!("the network {name} was dissolved") }).await;
        return;
    }
    // What may change the records is not taken while the hub hands over.
    let changes = matches!(
        intent,
        Intent::Session { .. }
            | Intent::Login { .. }
            | Intent::Token { .. }
            | Intent::Register { .. }
            | Intent::Probe
            | Intent::Handover { .. }
    );
    let _active = match changes.then(|| moving::Moving::enter(&hub)) {
        Some(None) => {
            refuse(&mut socket, Refusal::ShuttingDown).await;
            return;
        }
        Some(Some(active)) => Some(active),
        None => None,
    };
    match (path.as_str(), intent) {
        (
            wire::CONTROL_PATH,
            Intent::Session {
                replicated_records,
                key,
                proof,
                letters,
                through,
                direct,
                interfaces,
                any_names,
                dialed,
                renames,
                afoot,
                forgets,
                roles,
                versions,
                version_parts,
                due,
                project_deletes,
                project_entry,
                conversation_deletions,
                profiles,
                account_records,
                record_keys,
                skill_files,
                conversation_plans,
                scratch_locations,
                left,
                cancels,
                presence,
                parts,
                call_details,
                view_deltas,
            },
        ) => {
            let said = control::Said {
                replicated_records,
                through,
                direct,
                interfaces,
                any_names,
                dialed,
                renames,
                afoot,
                forgets,
                roles,
                versions,
                version_parts,
                due,
                project_deletes,
                project_entry,
                conversation_deletions,
                profiles,
                account_records,
                record_keys,
                skill_files,
                conversation_plans,
                scratch_locations,
                left,
                cancels,
                presence,
                parts,
                call_details,
                view_deltas,
            };
            control::session(hub, socket, peer, binding, key, proof, letters, said).await;
        }
        (wire::CONTROL_PATH, Intent::Login { account, request }) => {
            login::with_password(hub, socket, peer, binding, account, request).await;
        }
        (wire::CONTROL_PATH, Intent::Token { token, proof, member }) => {
            login::with_token(hub, socket, peer, binding, token, proof, member).await;
        }
        (wire::CONTROL_PATH, Intent::Peek { token, proof, member }) => {
            login::peek(hub, socket, peer, binding, token, proof, member).await;
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
        (wire::DATA_PATH, Intent::Package { key, proof, version, target, from }) => {
            updates::serve(hub, socket, binding, key, proof, version, target, from).await;
        }
        (wire::DATA_PATH, Intent::Deliver { key, proof, id }) => {
            updates::deliver(hub, socket, binding, key, proof, id).await;
        }
        (wire::CONTROL_PATH, Intent::Handover { id, role, proof }) => {
            moving::follow::old_connected(hub, socket, binding, id, role, proof).await;
        }
        _ => {
            hub.log.line(format_args!("{peer}: refused a hello that is not said on {path}"));
            refuse(&mut socket, Refusal::Broken { reason: format!("that is not said on {path}") }).await;
        }
    }
}

pub(crate) async fn send(socket: &mut Socket, frame: &Down) -> Result<(), String> {
    socket.send(WsMessage::Binary(wire::encode(frame).into())).await.map_err(|error| error.to_string())
}

/// Says why the hub will not go on, and closes; whether it was said. Neither
/// waits longer than the silence on a machine that does not read.
pub(crate) async fn refuse(socket: &mut Socket, reason: Refusal) -> bool {
    let said =
        tokio::time::timeout(SILENCE, send(socket, &Down::Refused { reason })).await.is_ok_and(|sent| sent.is_ok());
    let _ = tokio::time::timeout(SILENCE, socket.close(None)).await;
    said
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

/// How a session's machine is connected, as the hub sees it.
fn presence_of(session: &Session) -> views::Presence {
    let seen = session.through.is_none().then(|| session.peer.ip());
    views::Presence {
        since: Some(session.since),
        through: session.through.clone(),
        reach: fastctx_net::directory::Reach { seen, interfaces: session.interfaces.clone() },
        paths: session.paths.clone(),
    }
}
