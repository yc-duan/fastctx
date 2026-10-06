//! A machine's link to the hub, held by its node.
//!
//! The link keeps one control connection to the hub open for as long as the
//! node runs, connecting again whenever it drops. Over it the node calls
//! other machines and answers their calls, sends letters and takes the
//! letters kept for it, and asks the hub who the members are. Everything
//! another machine says is checked here before the node sees it: the
//! signature, that the sender is a member this machine trusts, that the
//! envelope is for this machine, and that a call is not one it took before.
//! What two members say to each other is sealed for them alone: calls and
//! answers on a session between the two (`sessions`), letters to the
//! receiver's key; the hub routes what it cannot read.

mod answering;
mod calls;
mod changes;
mod kernel;
mod ledger;
mod letters;
mod outgoing;
mod session;
mod sessions;
pub mod state;

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use fastctx_types::MachineName;
use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use jiff::Timestamp;
use tokio::sync::{Notify, mpsc, oneshot, watch};

use crate::code::Code;
use crate::data::Stream;
use crate::ledger::TownId;
use crate::message::LetterId;
use crate::scope::{Grant, Scope};
use crate::seal::SealKey;
use crate::sign::{Domain, PublicKey, SecretKey, Signed};
use crate::tree::{self, Endpoint, RelayOffer, Route, Upstream};
use crate::trust::{self, Endorsement, Kind, Member, Record, Revocation, Table};
use crate::wire::{CallStatus, Presence, Question, Told};
pub use ledger::{Page, PageItem};
use sessions::Sessions;
use state::{Counter, LettersFile, MembersFile, Taken};
pub use state::{Membership, NetDir};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What a caller does with its end of a call's data connection, the source.
pub type Feed = Box<dyn FnOnce(Stream) -> BoxFuture<Result<(), String>> + Send>;

/// What the link hands the node: calls to answer and letters to keep.
pub trait Node: Send + Sync + 'static {
    /// Admits a call and keeps its work alive through writing the reply,
    /// including a large reply's data transfer. Streams may return no guard.
    fn answering(&self, call: &RemoteCall) -> Result<Option<Box<dyn Send>>, fastctx_types::ipc::Failure>;
    /// Does what `from` asks and answers. A call that comes with a data
    /// connection hands over this machine's end of it, the sink.
    fn call(&self, from: MachineName, call: RemoteCall, stream: Option<Stream>) -> BoxFuture<RemoteResult>;
    /// Keeps a letter from `from`, which it named `id`. An inner error says
    /// why the letter cannot be taken, and it is set aside with that reason;
    /// an outer one is a failure of this machine, and the letter comes again,
    /// with the same id, as it may once more when this machine stops before
    /// it records having taken it.
    fn letter(&self, from: MachineName, id: LetterId, letter: Letter) -> BoxFuture<io::Result<Result<(), String>>>;
    /// The ledger of `town`, or of every town when `None`, may have items
    /// this machine does not have yet.
    fn ledger_changed(&self, town: Option<TownId>);
    /// The members, their scopes or who holds management rights may have
    /// changed: the link has just asked the hub.
    fn members_changed(&self) {}
    fn log(&self, line: String);
}

/// How the link stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not connected; trying. `last_error` says why the last try failed.
    Connecting { since: Timestamp, last_error: Option<String> },
    /// Connected at `address`, to the hub or to the relay `relay`; `route`
    /// says the way the connection leaves this machine.
    Online { since: Timestamp, address: SocketAddr, relay: Option<MachineName>, route: fastctx_types::world::Route },
    /// The hub explicitly revoked this machine, or the link was stopped
    /// locally. Other refusals remain retryable.
    Stopped { reason: String },
}

/// Why a call got no answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CallFailure {
    #[error("this machine is not connected to the hub ({reason}); nothing was sent")]
    NotConnected { reason: String },
    #[error("no machine of the World is named {name}; its machines are {}", machines.iter().map(MachineName::as_str).collect::<Vec<_>>().join(", "))]
    NotMember { name: MachineName, machines: Vec<MachineName> },
    #[error("{name} is offline{}", last_seen.map(|at| format!(", last seen {}", at.strftime("%Y-%m-%d %H:%M:%S UTC"))).unwrap_or_default())]
    Offline { name: MachineName, last_seen: Option<Timestamp> },
    #[error("{name} did not answer ({reason}); whether it did what was asked is not known")]
    Unknown { name: MachineName, reason: String },
    #[error("the call did not reach {name} ({reason}); nothing ran there")]
    Unreachable { name: MachineName, reason: String },
}

/// A machine's two keys: the one it signs with, and the one others seal to.
pub struct Keys {
    pub sign: SecretKey,
    pub seal: SealKey,
}

pub struct Link {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    dir: NetDir,
    membership: Membership,
    key: SecretKey,
    seal: SealKey,
    node: Arc<dyn Node>,
    counter: Counter,
    taken: Taken,
    sessions: Sessions,
    letters: Mutex<Letters>,
    members: Mutex<Members>,
    /// One refresh of the members at a time; others wait for it.
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
    /// Woken when the upstreams this machine should connect to changed, or
    /// a preferred one it is not on takes sessions again.
    rerouted: Notify,
    /// The relay on this machine, which its own link connects through
    /// first, so it and the machines below it speak without the root.
    local: Mutex<Option<Endpoint>>,
    /// Answers being written that must not lose their connection to a
    /// change of upstream they brought about (`Link::hold_reroute`).
    holding: Mutex<Holding>,
    /// The task that keeps the connection.
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct Letters {
    file: LettersFile,
    /// The number the next letter gets.
    next: u64,
}

struct Members {
    file: MembersFile,
    /// Replaced whole on each refresh, so a reader holds it as it was.
    table: Arc<Table>,
    presence: Vec<Presence>,
    /// When the latest invitation, grant or revocation the hub holds was made.
    latest: Option<Timestamp>,
}

#[derive(Default)]
struct Holding {
    holds: usize,
    /// A change of upstream came while held.
    pending: bool,
}

/// Holds off connecting again for a change of upstream until it is dropped
/// (`Link::hold_reroute`).
pub struct RerouteHold {
    inner: Arc<Inner>,
}

impl Drop for RerouteHold {
    fn drop(&mut self) {
        let pending = {
            let mut holding = self.inner.holding.lock().expect("never poisoned");
            holding.holds -= 1;
            holding.holds == 0 && std::mem::take(&mut holding.pending)
        };
        if pending {
            self.inner.reroute_if_changed();
        }
    }
}

/// The current connection: where to put frames to send on it, and the
/// upstream it is on, which data connections go to as well.
#[derive(Clone)]
pub(crate) struct Connection {
    generation: u64,
    up: outgoing::Outgoing,
    endpoint: Endpoint,
    /// The upstreams this machine tried, through the one it is on.
    tried: Vec<Endpoint>,
}

/// A call waiting for what answers it.
pub(crate) struct Waiter {
    events: mpsc::UnboundedSender<CallEvent>,
}

pub(crate) enum CallEvent {
    Status(CallStatus),
    /// The envelope the hub says answers the call, not yet checked.
    Answered(Signed),
    /// The connection the call went out on ended.
    Disconnected(String),
}

impl Link {
    /// Starts the link of the machine `membership` describes. The node's
    /// device key signs everything it says.
    pub fn start(dir: NetDir, membership: Membership, keys: Keys, node: Arc<dyn Node>) -> io::Result<Self> {
        let changes = changes::Changes::new(node.clone())?;
        Self::start_with_changes(dir, membership, keys, node, Some(changes))
    }

    /// Starts a link whose network and resume notifications are supplied by
    /// its application through [`Link::reconnect`]. Heartbeats and automatic
    /// retries still run here. Mobile applications own their OS callbacks.
    pub fn start_managed(dir: NetDir, membership: Membership, keys: Keys, node: Arc<dyn Node>) -> io::Result<Self> {
        Self::start_with_changes(dir, membership, keys, node, None)
    }

    fn start_with_changes(
        dir: NetDir,
        membership: Membership,
        keys: Keys,
        node: Arc<dyn Node>,
        changes: Option<changes::Changes>,
    ) -> io::Result<Self> {
        dir.prepare()?;
        let counter = Counter::open(dir.counter())?;
        let taken = Taken::open(dir.calls())?;
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
        let members_file: MembersFile = state::read_or(&dir.members(), MembersFile::default())?;
        let (table, _) = trust::check(
            &membership.world,
            &membership.founder,
            &members_file.records,
            &members_file.revocations,
            &members_file.grants,
            &members_file.routes,
            &members_file.known,
        );
        let (status, _) = watch::channel(Status::Connecting { since: Timestamp::now(), last_error: None });
        let inner = Arc::new(Inner {
            dir,
            membership,
            key: keys.sign,
            seal: keys.seal,
            node,
            counter,
            taken,
            sessions: Sessions::default(),
            letters: Mutex::new(Letters { file: letters_file, next }),
            members: Mutex::new(Members {
                file: members_file,
                table: Arc::new(table),
                presence: Vec::new(),
                latest: None,
            }),
            refreshing: tokio::sync::Mutex::new(()),
            status,
            connection: Mutex::new(None),
            calls: Mutex::new(HashMap::new()),
            asks: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            outbox_changed: Notify::new(),
            reconnect: Notify::new(),
            rerouted: Notify::new(),
            local: Mutex::new(None),
            holding: Mutex::new(Holding::default()),
            task: Mutex::new(None),
        });
        let task = tokio::spawn(session::run(inner.clone(), changes));
        *inner.task.lock().expect("never poisoned") = Some(task);
        Ok(Self { inner })
    }

    /// Invalidates the current connection or dial and skips the retry delay.
    /// A stopped or revoked membership is never restarted by this signal.
    pub fn reconnect(&self) {
        self.inner.reconnect.notify_one();
    }

    /// The relay on this machine, while it takes sessions: this machine's
    /// own link connects through it first.
    pub fn relay_here(&self, relay: Option<Endpoint>) {
        *self.inner.local.lock().expect("never poisoned") = relay;
        self.inner.reroute_if_changed();
    }

    /// The relay on this machine reached at `relay` no longer takes
    /// sessions; another that took its place stays.
    pub fn relay_gone(&self, relay: &Endpoint) {
        {
            let mut local = self.inner.local.lock().expect("never poisoned");
            if local.as_ref() != Some(relay) {
                return;
            }
            *local = None;
        }
        self.inner.reroute_if_changed();
    }

    /// Holds off connecting again for a change of upstream while the hold
    /// lives: for the answer to a call that changes this machine's own
    /// upstreams, as making it a relay does, which would otherwise end the
    /// connection it is to be written on.
    pub fn hold_reroute(&self) -> RerouteHold {
        self.inner.holding.lock().expect("never poisoned").holds += 1;
        RerouteHold { inner: self.inner.clone() }
    }

    /// The upstreams `name` dials, in order, as the members stand here.
    pub fn endpoints(&self, name: &MachineName) -> Vec<Endpoint> {
        tree::endpoints(&self.inner.members().table, &self.inner.membership, name)
    }

    /// Says in this machine's record that it relays, reached at `offer`, or
    /// that it no longer does.
    pub async fn offer_relay(&self, offer: Option<RelayOffer>) -> Result<(), String> {
        self.refresh().await?;
        let record = {
            let members = self.inner.members();
            let me = members
                .table
                .get(&self.inner.membership.name)
                .ok_or_else(|| format!("{} is not a member this machine trusts", self.inner.membership.name))?;
            if me.relay == offer {
                return Ok(());
            }
            Record { relay: offer, ..self.inner.own_record(&members)? }
        };
        let record = Signed::new(&self.inner.key, Domain::Record, &record);
        self.inner.ask(Question::Republish { record }).await?;
        self.refresh().await
    }

    /// Sets where the member `name` connects: through `via`, in order, as
    /// this machine, which must hold management rights.
    pub async fn route(&self, name: &MachineName, via: Vec<Upstream>) -> Result<(), String> {
        self.refresh().await?;
        let membership = &self.inner.membership;
        let at = self.inner.stamp();
        let route = {
            let members = self.inner.members();
            if !members.table.get(&membership.name).is_some_and(|me| me.manage) {
                return Err(format!("{} does not hold management rights", membership.name));
            }
            let member =
                members.table.get(name).ok_or_else(|| format!("{name} is not a member this machine trusts"))?;
            for upstream in &via {
                if let Upstream::Relay { machine } = upstream {
                    if machine == name || members.table.carries(name, machine) {
                        return Err(format!("{machine} connects through {name}: {name} cannot connect through it"));
                    }
                    if members.table.get(machine).is_none_or(|relay| !relay.kind.is_machine()) {
                        return Err(format!("{machine} is not a machine of the World"));
                    }
                }
            }
            Route {
                world: membership.world.clone(),
                member: name.clone(),
                key: member.key,
                via,
                version: members.table.route(name).map(|route| route.version).unwrap_or(0) + 1,
                by: membership.name.clone(),
                at,
            }
        };
        let route = Signed::new(&self.inner.key, Domain::Route, &route);
        self.inner.ask(Question::Route { route }).await?;
        self.refresh().await
    }

    /// Connection changes, including explicit revocation. The initial value
    /// is the current state; use `changed` to wait for a later one.
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

    /// Whether the essential connection task ended unexpectedly. An
    /// explicit revocation or local stop is a normal end.
    pub fn failed(&self) -> bool {
        let ended = self.inner.task.lock().expect("never poisoned").as_ref().is_some_and(|task| task.is_finished());
        ended && !matches!(self.status(), Status::Stopped { .. })
    }

    /// The members this machine trusts and whether each is connected, as
    /// last heard from the hub.
    pub fn members(&self) -> (Table, Vec<Presence>) {
        let members = self.inner.members();
        ((*members.table).clone(), members.presence.clone())
    }

    /// The members this machine trusts, as last checked.
    pub fn table(&self) -> Arc<Table> {
        self.inner.members().table.clone()
    }

    /// This machine as the members table has it.
    pub fn me(&self) -> Option<Member> {
        self.inner.members().table.get(&self.inner.membership.name).cloned()
    }

    /// Asks the hub who the members are now.
    pub async fn refresh(&self) -> Result<(), String> {
        self.inner.refresh().await
    }

    /// Proves this machine is on the connection whose TLS exporter is
    /// `binding`, as its relay does on the connections it opens upward.
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

    /// What `signed` says, if the member `from` signed it in `domain`.
    pub async fn unseal<T: serde::de::DeserializeOwned>(
        &self,
        from: &MachineName,
        domain: Domain,
        signed: &Signed,
    ) -> Result<T, String> {
        let key = self.inner.key_of(from).await.ok_or_else(|| format!("{from} is not a member this machine trusts"))?;
        signed.open(&key, domain).map_err(|error| format!("what came from {from}: {error}"))
    }

    /// Calls `to` and waits for its answer.
    pub async fn call(&self, to: &MachineName, call: RemoteCall) -> Result<RemoteResult, CallFailure> {
        self.inner.call(to, call, None).await
    }

    /// Calls `to` with a data connection, whose source end `feed` is given
    /// once the other end is there, while the call waits for its answer.
    pub async fn call_with_stream(
        &self,
        to: &MachineName,
        call: RemoteCall,
        feed: Feed,
    ) -> Result<RemoteResult, CallFailure> {
        self.inner.call(to, call, Some(feed)).await
    }

    /// Sends a letter to `to`. It is on this machine's disk when this
    /// returns, and goes to the hub whenever the link is up.
    pub fn send_letter(&self, to: &MachineName, id: LetterId, letter: Letter) -> io::Result<()> {
        self.inner.send_letter(to, id, letter)
    }

    /// The most a letter to `to` takes as a control message, with the
    /// longest id and count it could carry: it can be sent when this is at
    /// most `wire::MAX_CONTROL_FRAME`.
    pub fn letter_size(&self, to: &MachineName, letter: &Letter) -> usize {
        self.inner.letter_size(to, letter)
    }

    /// An invitation for one more member, valid until `expires`: a machine,
    /// or a device such as a phone; `name` is the name it must take, if
    /// given; `scope` what it may do. Not given, it may do everything this
    /// machine may give, and never invite: that right is given only on
    /// purpose.
    pub async fn invite(
        &self,
        name: Option<MachineName>,
        expires: Timestamp,
        kind: Kind,
        scope: Option<Scope>,
    ) -> Result<Code, String> {
        self.refresh().await?;
        let scope = {
            let members = self.inner.members();
            let me = members
                .table
                .get(&self.inner.membership.name)
                .ok_or_else(|| format!("{} is not a member this machine trusts", self.inner.membership.name))?;
            let scope = scope.unwrap_or_else(|| {
                let mut scope = if me.manage { Scope::all() } else { me.scope.clone() };
                scope.invite = false;
                scope
            });
            members.table.may_invite(&self.inner.membership.name, &scope)?;
            Some(scope)
        };
        let secret = crate::code::InviteSecret::generate().map_err(|error| error.to_string())?;
        let invite = secret.key();
        let membership = &self.inner.membership;
        let endorsement = Endorsement {
            world: membership.world.clone(),
            inviter: membership.name.clone(),
            invite: invite.public(),
            expires,
            name,
            kind,
            scope,
            at: Some(self.inner.stamp()),
        };
        let endorsement = Signed::new(&self.inner.key, Domain::Endorsement, &endorsement);
        self.inner.ask(Question::Invite { endorsement }).await?;
        Ok(Code::Invitation {
            hub: membership.hub.clone(),
            cert: membership.cert,
            world: membership.world.clone(),
            founder: membership.founder,
            secret,
        })
    }

    /// Revokes the member `name`'s key; this machine's own, to leave.
    pub async fn revoke(&self, name: &MachineName, reason: &str) -> Result<(), String> {
        self.refresh().await?;
        let membership = &self.inner.membership;
        let key = {
            let members = self.inner.members();
            let key = match members.table.get(name) {
                Some(member) => member.key,
                None => return Err(format!("{name} is not a member this machine trusts")),
            };
            if !members.table.may_revoke(&membership.name, name) {
                return Err(format!(
                    "{} may revoke only itself and the members of its own branch, unless it holds management rights",
                    membership.name
                ));
            }
            key
        };
        let revocation = Revocation {
            world: membership.world.clone(),
            name: name.clone(),
            key,
            by: membership.name.clone(),
            at: self.inner.stamp(),
            reason: reason.to_string(),
        };
        let revocation = Signed::new(&self.inner.key, Domain::Revocation, &revocation);
        let answer = self.inner.ask(Question::Revoke { revocation }).await;
        if *name == membership.name && matches!(self.status(), Status::Stopped { .. }) {
            return Ok(());
        }
        answer?;
        Ok(())
    }

    /// Sets what the member `name` may do, and whether it holds management
    /// rights, as this machine, which must hold them.
    pub async fn grant(&self, name: &MachineName, scope: Scope, manage: bool) -> Result<(), String> {
        self.refresh().await?;
        let membership = &self.inner.membership;
        let at = self.inner.stamp();
        let grant = {
            let members = self.inner.members();
            if !members.table.get(&membership.name).is_some_and(|me| me.manage) {
                return Err(format!("{} does not hold management rights", membership.name));
            }
            let member =
                members.table.get(name).ok_or_else(|| format!("{name} is not a member this machine trusts"))?;
            if manage && !member.kind.is_machine() {
                return Err(format!("{name} is a device, and a device holds no management rights"));
            }
            Grant {
                world: membership.world.clone(),
                member: name.clone(),
                key: member.key,
                scope,
                manage,
                version: members.table.grant(name).map(|grant| grant.version).unwrap_or(0) + 1,
                by: membership.name.clone(),
                at,
            }
        };
        let grant = Signed::new(&self.inner.key, Domain::Grant, &grant);
        self.inner.ask(Question::Grant { grant }).await?;
        self.refresh().await
    }
}

impl Inner {
    fn members(&self) -> MutexGuard<'_, Members> {
        self.members.lock().expect("never poisoned")
    }

    /// The time to sign into a new invitation, grant or revocation: now, or
    /// just after the latest the hub holds when this machine's clock is
    /// behind it (`wire::Told::Members`).
    fn stamp(&self) -> Timestamp {
        let now = Timestamp::now();
        let latest = self.members().latest;
        match latest {
            // A latest the hub dates beyond any clock is refused by it as
            // such; this machine signs no time it cannot write.
            Some(latest) if latest >= now => latest.checked_add(jiff::SignedDuration::from_micros(1)).unwrap_or(latest),
            _ => now,
        }
    }

    fn connection(&self) -> Option<Connection> {
        self.connection.lock().expect("never poisoned").clone()
    }

    /// The upstreams this machine dials, in order: the relay on this
    /// machine, then where its route leads.
    fn endpoints(&self) -> Vec<Endpoint> {
        let mut endpoints: Vec<Endpoint> = self.local.lock().expect("never poisoned").iter().cloned().collect();
        endpoints.extend(tree::endpoints(&self.members().table, &self.membership, &self.membership.name));
        endpoints
    }

    /// Where a data connection goes: the upstream the control connection is
    /// on, for the two ends meet at their nearest common upstream.
    fn data_endpoint(&self) -> Endpoint {
        match self.connection() {
            Some(connection) => connection.endpoint,
            None => self.endpoints().into_iter().next().unwrap_or_else(|| Endpoint::hub(&self.membership)),
        }
    }

    /// Ends the connection when the upstream it is on is no longer one to
    /// connect through, or one this machine never tried comes before it.
    fn reroute_if_changed(&self) {
        let Some(connection) = self.connection() else { return };
        {
            let mut holding = self.holding.lock().expect("never poisoned");
            if holding.holds > 0 {
                holding.pending = true;
                return;
            }
        }
        if tree::moved(&self.endpoints(), &connection.endpoint, &connection.tried) {
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

    fn not_connected(&self) -> String {
        match &*self.status.borrow() {
            Status::Connecting { last_error: Some(error), .. } => error.clone(),
            Status::Connecting { last_error: None, .. } => "it is connecting".to_string(),
            Status::Online { .. } => "the connection just ended".to_string(),
            Status::Stopped { reason } => reason.clone(),
        }
    }

    /// The member `name`, asking the hub again when it is not known here
    /// yet.
    async fn member(&self, name: &MachineName) -> Option<Member> {
        if let Some(member) = self.members().table.get(name) {
            return Some(member.clone());
        }
        if self.refresh().await.is_err() {
            return None;
        }
        self.members().table.get(name).cloned()
    }

    /// A call to `name`, which is no member, refused with the machines there are.
    fn not_member(&self, name: &MachineName) -> CallFailure {
        let machines = self.members().table.machines().map(|member| member.name.clone()).collect();
        CallFailure::NotMember { name: name.clone(), machines }
    }

    async fn key_of(&self, name: &MachineName) -> Option<PublicKey> {
        self.member(name).await.map(|member| member.key)
    }

    async fn refresh(&self) -> Result<(), String> {
        let _one = self.refreshing.lock().await;
        let Told::Members { records, revocations, grants, routes, presence, latest } =
            self.ask(Question::Members).await?
        else {
            return Err("the hub answered something else".to_string());
        };
        let mut members = self.members();
        let (table, known) = trust::check(
            &self.membership.world,
            &self.membership.founder,
            &records,
            &revocations,
            &grants,
            &routes,
            &members.file.known,
        );
        let changed = known != members.file.known
            || records != members.file.records
            || revocations != members.file.revocations
            || grants != members.file.grants
            || routes != members.file.routes;
        if changed {
            let refused: Vec<String> = table.refused.iter().map(|(what, why)| format!("{what}: {why}")).collect();
            if !refused.is_empty() {
                self.node.log(format!("refused from the hub's list of members: {}", refused.join("; ")));
            }
            members.file = MembersFile { known, records, revocations, grants, routes };
            state::write(&self.dir.members(), &members.file)
                .map_err(|error| format!("keeping the members: {error}"))?;
        }
        for (name, _) in table.revoked_members() {
            self.sessions.drop_member(name);
        }
        members.table = Arc::new(table);
        members.presence = presence;
        members.latest = latest;
        drop(members);
        self.reroute_if_changed();
        Ok(())
    }

    /// This machine's record as the hub's list holds it.
    fn own_record(&self, members: &Members) -> Result<Record, String> {
        let me = &self.membership.name;
        members
            .file
            .records
            .iter()
            .filter_map(|signed| signed.claim::<Record>().ok())
            .find(|record| record.name == *me && record.key == self.key.public())
            .ok_or_else(|| format!("the hub's list has no record of {me} with this machine's key"))
    }

    /// Gives the hub this machine's record again with its sealing key, when
    /// the hub's has none or another.
    async fn republish(&self) -> Result<(), String> {
        let record = {
            let members = self.members();
            let me = &self.membership.name;
            if members.table.get(me).is_some_and(|member| member.seal == Some(self.seal.public())) {
                return Ok(());
            }
            let ours = self.own_record(&members)?;
            if ours.seal.is_some_and(|seal| seal != self.seal.public()) {
                return Err(format!(
                    "the hub's list has another sealing key for {me}: this machine lost the one it made, and must join the World again"
                ));
            }
            Record { seal: Some(self.seal.public()), ..ours }
        };
        let record = Signed::new(&self.key, Domain::Record, &record);
        self.ask(Question::Republish { record }).await?;
        self.refresh().await
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
}
