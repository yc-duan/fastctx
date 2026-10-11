//! A machine's link to one hub, held by its node.
//!
//! The link keeps one control connection to the hub open for as long as the
//! node runs, connecting again whenever it drops, through the relays its
//! view lists and then the hub itself. Over it the node calls other
//! machines and answers their calls, sends letters and takes the letters
//! kept for it, and asks the hub for its view of the directory and to make
//! changes for an account. Everything another machine says is checked here
//! against the view before the node sees it: that it is for this machine,
//! and that the directory lets the sender say it for the account it names
//! (`crate::directory`). The node checks again what each call asks.

mod accounts;
mod answering;
mod calls;
mod changes;
mod collab;
mod kernel;
mod ledger;
mod letters;
mod machine_log;
mod outgoing;
mod session;
pub mod state;

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use fastctx_types::access::{Purpose, TownId};
use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use tokio::sync::{Notify, mpsc, oneshot, watch};

use crate::data::{Endpoint, Stream};
use crate::directory::View;
use crate::envelope::{Envelope, Kind};
use crate::message::LetterId;
use crate::sign::{Domain, PublicKey, SecretKey, Signed};
use crate::wire::{CallStatus, Change, Question, Told};
pub use ledger::{Page, PageItem};
use state::{Counter, LettersFile};
pub use state::{Membership, NetDir};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What a caller does with its end of a call's data connection, the source.
pub type Feed = Box<dyn FnOnce(Stream) -> BoxFuture<Result<(), String>> + Send>;

/// Who says something to this machine: the member it comes from, and the
/// account it speaks for there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub machine: MachineName,
    pub account: AccountName,
}

/// What the link hands the node: calls to answer and letters to keep.
pub trait Node: Send + Sync + 'static {
    /// Admits a call and keeps its work alive through writing the reply,
    /// including a large reply's data transfer. Streams may return no guard.
    fn answering(&self, call: &RemoteCall) -> Result<Option<Box<dyn Send>>, fastctx_types::ipc::Failure>;
    /// Does what `from` asks and answers. A call that comes with a data
    /// connection hands over this machine's end of it, the sink.
    fn call(&self, from: Caller, call: RemoteCall, stream: Option<Stream>) -> BoxFuture<RemoteResult>;
    /// Keeps a letter from `from`, which it named `id` and sent for
    /// `purpose`. An inner error says why the letter cannot be taken, and it
    /// is set aside with that reason; an outer one is a failure of this
    /// machine, and the letter comes again, with the same id, as it may once
    /// more when this machine stops before it records having taken it.
    fn letter(
        &self,
        from: Caller,
        purpose: Purpose,
        id: LetterId,
        letter: Letter,
    ) -> BoxFuture<io::Result<Result<(), String>>>;
    /// The ledger of `town`, or of every project when `None`, may have
    /// entries this machine does not have yet.
    fn ledger_changed(&self, town: Option<TownId>);
    /// This machine's view of the directory may have changed: the link has
    /// just asked the hub.
    fn directory_changed(&self) {}
    fn log(&self, line: String);
}

/// How the link stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not connected; trying. `last_error` says why the last try failed.
    Connecting { since: Timestamp, last_error: Option<String> },
    /// Connected at `address`, to the hub or through the relay `relay`;
    /// `route` says the way the connection leaves this machine.
    Online { since: Timestamp, address: SocketAddr, relay: Option<String>, route: fastctx_types::world::Route },
    /// The hub removed this machine, or the link was stopped locally.
    /// Other refusals remain retryable.
    Stopped { reason: String },
}

/// Why a call got no answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CallFailure {
    #[error("this machine is not connected to the hub now ({reason}); nothing was sent")]
    NotConnected { reason: String },
    #[error("there is no machine named {name}; the machines are {}", machines.iter().map(MachineName::as_str).collect::<Vec<_>>().join(", "))]
    NotMember { name: MachineName, machines: Vec<MachineName> },
    #[error("{name} is offline{}", last_seen.map(offline_for).unwrap_or_default())]
    Offline { name: MachineName, last_seen: Option<Timestamp> },
    #[error("the hub did not pass the call to {name}: {reason}; nothing ran there")]
    Refused { name: MachineName, reason: String },
    #[error("the call to {name} was not made: {reason}; nothing ran there")]
    NotAllowed { name: MachineName, reason: String },
    #[error("{name} did not answer ({reason}); whether it did what was asked is not known")]
    Unknown { name: MachineName, reason: String },
    #[error("the call did not reach {name} ({reason}); nothing ran there")]
    Unreachable { name: MachineName, reason: String },
}

/// How long a machine has been offline, as `fastctx nodes` writes it: its
/// reader knows no machine's clock or zone.
fn offline_for(last_seen: Timestamp) -> String {
    let seconds = Timestamp::now().as_second().saturating_sub(last_seen.as_second()).max(0);
    let span = match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => match seconds % 3600 / 60 {
            0 => format!("{}h", seconds / 3600),
            minutes => format!("{}h{minutes}m", seconds / 3600),
        },
        _ => match seconds % 86_400 / 3600 {
            0 => format!("{}d", seconds / 86_400),
            hours => format!("{}d{hours}h", seconds / 86_400),
        },
    };
    format!(" for {span}")
}

/// A way to the hub: through a relay, or to the hub itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Upstream {
    /// The relay's label; `None` for the hub.
    pub relay: Option<String>,
    pub addresses: Vec<SocketAddr>,
}

impl std::fmt::Display for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.relay {
            Some(label) => write!(f, "the relay {label}"),
            None => f.write_str("the hub"),
        }
    }
}

pub struct Link {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    dir: NetDir,
    membership: Membership,
    key: SecretKey,
    node: Arc<dyn Node>,
    counter: Counter,
    letters: Mutex<Letters>,
    /// Replaced whole on each refresh, so a reader holds it as it was.
    view: Mutex<Arc<View>>,
    /// One refresh of the view at a time; others wait for it.
    refreshing: tokio::sync::Mutex<()>,
    status: watch::Sender<Status>,
    connection: Mutex<Option<Connection>>,
    calls: Mutex<HashMap<u64, Waiter>>,
    asks: Mutex<HashMap<u64, oneshot::Sender<Result<Told, String>>>>,
    next_id: AtomicU64,
    /// Woken when a letter is put in the outbox.
    outbox_changed: Notify,
    /// An application knows when its network changed or it returned to the foreground.
    reconnect: Notify,
    /// Woken when the ways to the hub this machine should connect by changed.
    rerouted: Notify,
    /// The task that keeps the connection.
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct Letters {
    file: LettersFile,
    /// The number the next letter gets.
    next: u64,
}

/// The current connection: where to put frames to send on it, and the way
/// to the hub it is on, which data connections take first.
#[derive(Clone)]
pub(crate) struct Connection {
    generation: u64,
    up: outgoing::Outgoing,
    upstream: Upstream,
    address: SocketAddr,
    /// The ways this machine tried, through the one it is on.
    tried: Vec<Upstream>,
}

/// A call waiting for what answers it.
pub(crate) struct Waiter {
    events: mpsc::UnboundedSender<CallEvent>,
}

pub(crate) enum CallEvent {
    Status(CallStatus),
    /// The envelope the hub says answers the call, not yet checked.
    Answered(Envelope),
    /// The connection the call went out on ended.
    Disconnected(String),
}

impl Link {
    /// Starts the link of the machine `membership` describes, which says
    /// who it is with `key`.
    pub fn start(dir: NetDir, membership: Membership, key: SecretKey, node: Arc<dyn Node>) -> io::Result<Self> {
        let changes = changes::Changes::new(node.clone())?;
        Self::start_with_changes(dir, membership, key, node, Some(changes))
    }

    /// Starts a link whose network and resume notifications are supplied by
    /// its application through [`Link::reconnect`]. Heartbeats and automatic
    /// retries still run here. Mobile applications own their OS callbacks.
    pub fn start_managed(dir: NetDir, membership: Membership, key: SecretKey, node: Arc<dyn Node>) -> io::Result<Self> {
        Self::start_with_changes(dir, membership, key, node, None)
    }

    fn start_with_changes(
        dir: NetDir,
        membership: Membership,
        key: SecretKey,
        node: Arc<dyn Node>,
        changes: Option<changes::Changes>,
    ) -> io::Result<Self> {
        dir.prepare()?;
        let counter = Counter::open(dir.counter())?;
        let letters_file = match state::read::<LettersFile>(&dir.letters()) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let file = LettersFile { epoch: u64::from_le_bytes(crate::sign::random::<8>()?), held: 0, taken: None };
                state::write(&dir.letters(), &file)?;
                file
            }
            Err(error) => return Err(error),
        };
        let highest = letters::Outbox::new(&dir).numbers()?.last().copied().unwrap_or(0);
        let next = letters_file.held.max(highest) + 1;
        let view = match state::read::<View>(&dir.view()) {
            Ok(view) if view.hub == membership.id && view.me == membership.name => view,
            Ok(_) => View::empty(membership.id.clone(), membership.hub_name.clone(), membership.name.clone()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                View::empty(membership.id.clone(), membership.hub_name.clone(), membership.name.clone())
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData && state::older_view(&dir.view()) => {
                node.log(format!(
                    "the view of the hub {} kept here was given by an older hub; the hub gives it again",
                    membership.hub_name
                ));
                View::empty(membership.id.clone(), membership.hub_name.clone(), membership.name.clone())
            }
            Err(error) => return Err(error),
        };
        let (status, _) = watch::channel(Status::Connecting { since: Timestamp::now(), last_error: None });
        let inner = Arc::new(Inner {
            dir,
            membership,
            key,
            node,
            counter,
            letters: Mutex::new(Letters { file: letters_file, next }),
            view: Mutex::new(Arc::new(view)),
            refreshing: tokio::sync::Mutex::new(()),
            status,
            connection: Mutex::new(None),
            calls: Mutex::new(HashMap::new()),
            asks: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            outbox_changed: Notify::new(),
            reconnect: Notify::new(),
            rerouted: Notify::new(),
            task: Mutex::new(None),
        });
        let task = tokio::spawn(session::run(inner.clone(), changes));
        *inner.task.lock().expect("never poisoned") = Some(task);
        Ok(Self { inner })
    }

    /// Invalidates the current connection or dial and skips the retry delay.
    /// A stopped or removed membership is never restarted by this signal.
    pub fn reconnect(&self) {
        self.inner.reconnect.notify_one();
    }

    /// Connection changes, including removal. The initial value is the
    /// current state; use `changed` to wait for a later one.
    pub fn subscribe(&self) -> watch::Receiver<Status> {
        self.inner.status.subscribe()
    }

    /// Closes the connection and stops connecting.
    pub fn stop(&self) {
        if let Some(task) = self.inner.task.lock().expect("never poisoned").take() {
            task.abort();
        }
        *self.inner.connection.lock().expect("never poisoned") = None;
        self.inner.status.send_replace(Status::Stopped { reason: "the link was stopped".to_string() });
    }

    pub fn membership(&self) -> &Membership {
        &self.inner.membership
    }

    pub fn status(&self) -> Status {
        self.inner.status.borrow().clone()
    }

    /// Whether the essential connection task ended unexpectedly. A removal
    /// or a local stop is a normal end.
    pub fn failed(&self) -> bool {
        let ended = self.inner.task.lock().expect("never poisoned").as_ref().is_some_and(|task| task.is_finished());
        ended && !matches!(self.status(), Status::Stopped { .. })
    }

    /// This machine's view of the directory, as last given by the hub.
    pub fn view(&self) -> Arc<View> {
        self.inner.view()
    }

    /// The hub's short name, as the hub last said it.
    pub fn hub_name(&self) -> fastctx_types::HubName {
        self.inner.view().hub_name.clone()
    }

    /// Asks the hub for this machine's view again.
    pub async fn refresh(&self) -> Result<(), String> {
        self.inner.refresh().await
    }

    /// Where a data connection of this machine goes.
    pub fn data_endpoint(&self) -> Endpoint {
        self.inner.data_endpoint()
    }

    /// Proves this machine is on the connection whose TLS exporter is
    /// `binding`.
    pub fn prove(&self, binding: &[u8; 32]) -> crate::sign::Signature {
        self.inner.key.sign(Domain::Session, binding)
    }

    pub fn public(&self) -> PublicKey {
        self.inner.key.public()
    }

    /// Signs `statement` as this machine, in `domain`.
    pub fn sign(&self, domain: Domain, statement: &impl serde::Serialize) -> Signed {
        Signed::new(&self.inner.key, domain, statement)
    }

    /// Signs `bytes` as they are, as this machine, in `domain`.
    pub fn sign_bytes(&self, domain: Domain, bytes: Vec<u8>) -> Signed {
        Signed::of_bytes(&self.inner.key, domain, bytes)
    }

    /// What `signed` says, packed or not (`crate::history::unpack`), if the
    /// member `from` signed it in `domain`. It is unpacked off the runtime's
    /// threads: a long history's segments take seconds to unpack and read.
    pub async fn unseal_packed<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        from: &MachineName,
        domain: Domain,
        signed: &Signed,
    ) -> Result<T, String> {
        let key = self.inner.key_of(from).await.ok_or_else(|| format!("{from} is not a member of the hub"))?;
        let bytes = signed.verified(&key, domain).map_err(|error| format!("what came from {from}: {error}"))?.to_vec();
        tokio::task::spawn_blocking(move || crate::history::unpack(&bytes))
            .await
            .map_err(|error| format!("unpacking what came from {from}: {error}"))?
            .map_err(|error| format!("what came from {from}: {error}"))
    }

    /// What `signed` says, if the member `from` signed it in `domain`.
    pub async fn unseal<T: serde::de::DeserializeOwned>(
        &self,
        from: &MachineName,
        domain: Domain,
        signed: &Signed,
    ) -> Result<T, String> {
        let key = self.inner.key_of(from).await.ok_or_else(|| format!("{from} is not a member of the hub"))?;
        signed.open(&key, domain).map_err(|error| format!("what came from {from}: {error}"))
    }

    /// Calls `to` for `account`, asking `purpose` of it, and waits for its
    /// answer.
    pub async fn call(
        &self,
        to: &MachineName,
        account: &AccountName,
        purpose: Purpose,
        call: RemoteCall,
    ) -> Result<RemoteResult, CallFailure> {
        self.inner.call(to, account, purpose, call, None).await
    }

    /// Calls `to` with a data connection, whose source end `feed` is given
    /// once the other end is there, while the call waits for its answer.
    pub async fn call_with_stream(
        &self,
        to: &MachineName,
        account: &AccountName,
        purpose: Purpose,
        call: RemoteCall,
        feed: Feed,
    ) -> Result<RemoteResult, CallFailure> {
        self.inner.call(to, account, purpose, call, Some(feed)).await
    }

    /// Sends a letter to `to`, for `account`. It is on this machine's disk
    /// when this returns, and goes to the hub whenever the link is up.
    pub fn send_letter(
        &self,
        to: &MachineName,
        account: &AccountName,
        purpose: Purpose,
        id: LetterId,
        letter: Letter,
    ) -> io::Result<()> {
        self.inner.send_letter(to, account, purpose, id, letter)
    }

    /// The most a letter to `to` takes as a control message, with the
    /// longest id and count it could carry: it can be sent when this is at
    /// most `wire::MAX_CONTROL_FRAME`.
    pub fn letter_size(&self, to: &MachineName, account: &AccountName, letter: &Letter) -> usize {
        self.inner.letter_size(to, account, letter)
    }

    /// Makes `change` as `account`, which must be logged in here, and takes
    /// the view the change leaves. The change is made once the hub says so;
    /// a change to this machine's own ways to the hub ends the connection
    /// the view would have come by, and the next one takes it.
    pub async fn manage(&self, account: &AccountName, change: Change) -> Result<Told, String> {
        let told = self.inner.ask(Question::Manage { account: account.clone(), change }).await?;
        if let Err(error) = self.inner.refresh().await {
            self.inner.node.log(format!("asking the hub for the view after a change: {error}"));
        }
        Ok(told)
    }
}

impl Inner {
    fn view(&self) -> Arc<View> {
        self.view.lock().expect("never poisoned").clone()
    }

    fn connection(&self) -> Option<Connection> {
        self.connection.lock().expect("never poisoned").clone()
    }

    /// The ways to the hub, in order: the relays this machine's view lists
    /// for it, then the hub itself.
    fn upstreams(&self) -> Vec<Upstream> {
        let view = self.view();
        let mut upstreams: Vec<Upstream> = view
            .me()
            .map(|me| {
                me.relays
                    .iter()
                    .map(|relay| Upstream { relay: Some(relay.label.clone()), addresses: relay.addresses.clone() })
                    .collect()
            })
            .unwrap_or_default();
        upstreams.push(Upstream { relay: None, addresses: self.membership.addresses.clone() });
        upstreams
    }

    /// Where a data connection goes: the address the control connection is
    /// on, then every other way.
    fn data_endpoint(&self) -> Endpoint {
        let mut addresses = Vec::new();
        if let Some(connection) = self.connection() {
            addresses.push(connection.address);
        }
        for upstream in self.upstreams() {
            for address in upstream.addresses {
                if !addresses.contains(&address) {
                    addresses.push(address);
                }
            }
        }
        Endpoint { addresses, cert: self.membership.cert }
    }

    /// Ends the connection when the way it is on is no longer one to
    /// connect by, or one this machine never tried comes before it.
    fn reroute_if_changed(&self) {
        let Some(connection) = self.connection() else { return };
        let upstreams = self.upstreams();
        let Some(position) = upstreams.iter().position(|upstream| *upstream == connection.upstream) else {
            self.rerouted.notify_waiters();
            return;
        };
        if upstreams[..position].iter().any(|upstream| !connection.tried.contains(upstream)) {
            self.rerouted.notify_waiters();
        }
    }

    /// The current connection; while the link's first attempt to connect is
    /// still under way, as just after the node started, the outcome of that
    /// attempt.
    async fn connected(&self) -> Option<Connection> {
        let mut status = self.status.subscribe();
        loop {
            if let Some(connection) = self.connection() {
                return Some(connection);
            }
            if !matches!(*status.borrow_and_update(), Status::Connecting { last_error: None, .. }) {
                return self.connection();
            }
            if status.changed().await.is_err() {
                return None;
            }
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Why a call found no connection, for whoever made the call, agents
    /// included: the dialing error names the hub's addresses, which stay in
    /// the node's log and the link's status for the people who look there.
    fn not_connected(&self) -> String {
        match &*self.status.borrow() {
            Status::Connecting { .. } => "it is reconnecting".to_string(),
            Status::Online { .. } => "the connection just ended".to_string(),
            Status::Stopped { reason } => reason.clone(),
        }
    }

    /// The key of the member `name`, asking the hub again when it is not
    /// in the view yet.
    async fn key_of(&self, name: &MachineName) -> Option<PublicKey> {
        if let Some(member) = self.view().member(name) {
            return Some(member.key);
        }
        if self.refresh().await.is_err() {
            return None;
        }
        self.view().member(name).map(|member| member.key)
    }

    /// A call to `name`, which is no member, refused with the machines there are.
    fn not_member(&self, name: &MachineName) -> CallFailure {
        let machines = self.view().machines().map(|member| member.name.clone()).collect();
        CallFailure::NotMember { name: name.clone(), machines }
    }

    async fn refresh(&self) -> Result<(), String> {
        let _one = self.refreshing.lock().await;
        let Told::Directory { view } = self.ask(Question::Directory).await? else {
            return Err("the hub answered something else".to_string());
        };
        if view.hub != self.membership.id || view.me != self.membership.name {
            return Err(format!(
                "the hub gave the view of {} on {}, not of {} on {}",
                view.me, view.hub, self.membership.name, self.membership.id
            ));
        }
        let changed = *self.view() != *view;
        if changed {
            state::write(&self.dir.view(), &*view).map_err(|error| format!("keeping the view: {error}"))?;
            *self.view.lock().expect("never poisoned") = Arc::from(view);
        }
        self.reroute_if_changed();
        Ok(())
    }

    /// Checks that `envelope` is for this machine and that the view lets
    /// its sender say it, for the account it names, asking the hub for the
    /// view again once when it does not: a grant the hub just made may not
    /// be in the view yet.
    async fn admit(&self, envelope: &Envelope) -> Result<Caller, String> {
        if envelope.to != self.membership.name || envelope.hub != self.membership.id {
            return Err(format!("it is for {} on {}", envelope.to, envelope.hub));
        }
        let (account, purpose) = envelope.standing()?;
        let me = &self.membership.name;
        let check = |view: &View| match (envelope.kind, purpose) {
            (Kind::Letter, Purpose::Echo) => view.admits_echo(&envelope.from, account, me),
            _ => view.admits(&envelope.from, account, me, purpose),
        };
        if check(&self.view()).is_err() {
            if let Err(error) = self.refresh().await {
                self.node.log(format!("asking the hub for the view: {error}"));
            }
            check(&self.view())?;
        }
        Ok(Caller { machine: envelope.from.clone(), account: account.clone() })
    }

    async fn ask(&self, question: Question) -> Result<Told, String> {
        let connection =
            self.connected().await.ok_or_else(|| format!("not connected to the hub: {}", self.not_connected()))?;
        self.ask_on(connection, question)
            .await?
            .ok_or_else(|| "the connection to the hub ended before it answered".to_string())
    }

    /// None means that the connection ended, distinct from a hub refusal.
    async fn ask_on(&self, connection: Connection, question: Question) -> Result<Option<Told>, String> {
        let id = self.next_id();
        let (answer, answered) = oneshot::channel();
        self.asks.lock().expect("never poisoned").insert(id, answer);
        struct Waiting<'a>(&'a Inner, u64);
        impl Drop for Waiting<'_> {
            fn drop(&mut self) {
                let mut asks = self.0.asks.lock().expect("never poisoned");
                asks.remove(&self.1);
                if asks.is_empty() {
                    *asks = Default::default();
                }
            }
        }
        let _waiting = Waiting(self, id);
        if connection.up.send(crate::wire::Up::Ask { id, question }).is_err() {
            self.asks.lock().expect("never poisoned").remove(&id);
            return Ok(None);
        }
        // The answer, or the end of the connection, which drops the sender.
        match answered.await {
            Ok(answer) => answer.map(Some),
            Err(_) => Ok(None),
        }
    }

    fn letters(&self) -> MutexGuard<'_, Letters> {
        self.letters.lock().expect("never poisoned")
    }
}
