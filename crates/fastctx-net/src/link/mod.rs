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
mod ledger_query;
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

use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;
use tokio::sync::{Notify, mpsc, oneshot, watch};

use crate::code::Code;
use crate::data::Stream;
use crate::message::LetterId;
use crate::scope::{Grant, Scope};
use crate::seal::SealKey;
use crate::sign::{Domain, PublicKey, SecretKey, Signed};
use crate::trust::{self, Endorsement, Kind, Member, Record, Revocation, Table};
use crate::wire::{CallStatus, Presence, Question, Told};
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
    /// The ledger of `town`, or of every town when `None`, may have entries
    /// this machine does not have yet.
    fn ledger_changed(&self, town: Option<TownName>);
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
    /// Connected to the hub at `address`; `route` says the way the
    /// connection leaves this machine.
    Online { since: Timestamp, address: SocketAddr, route: fastctx_types::world::Route },
    /// The hub explicitly revoked this machine, or the link was stopped
    /// locally. Other refusals remain retryable.
    Stopped { reason: String },
}

/// Why a call got no answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CallFailure {
    #[error("this machine is not connected to the hub ({reason}); nothing was sent")]
    NotConnected { reason: String },
    #[error("no member of the World is named {0}")]
    NotMember(MachineName),
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
    table: Table,
    presence: Vec<Presence>,
}

/// The current connection: where to put frames to send on it.
#[derive(Clone)]
pub(crate) struct Connection {
    generation: u64,
    up: outgoing::Outgoing,
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
            members: Mutex::new(Members { file: members_file, table, presence: Vec::new() }),
            refreshing: tokio::sync::Mutex::new(()),
            status,
            connection: Mutex::new(None),
            calls: Mutex::new(HashMap::new()),
            asks: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            outbox_changed: Notify::new(),
            reconnect: Notify::new(),
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
        (members.table.clone(), members.presence.clone())
    }

    /// This machine as the members table has it.
    pub fn me(&self) -> Option<Member> {
        self.inner.members().table.get(&self.inner.membership.name).cloned()
    }

    /// Asks the hub who the members are now.
    pub async fn refresh(&self) -> Result<(), String> {
        self.inner.refresh().await
    }

    /// Writes an entry into its town's ledger at the hub, signed as this
    /// machine. Returns its number there.
    pub async fn ledger_write(&self, entry: &crate::ledger::Entry) -> Result<u64, String> {
        let signed = crate::ledger::sign(&self.inner.key, entry);
        match self.inner.ask(Question::LedgerWrite { entry: signed }).await? {
            Told::Written { seq } => Ok(seq),
            _ => Err("the hub answered something else".to_string()),
        }
    }

    /// The towns this machine spans, with the number of each one's latest
    /// entry at the hub.
    pub async fn ledger_towns(&self) -> Result<Vec<(TownName, u64)>, String> {
        match self.inner.ask(Question::LedgerTowns).await? {
            Told::Towns { towns } => Ok(towns),
            _ => Err("the hub answered something else".to_string()),
        }
    }

    /// The entries of `town` after `after`, as signed by their authors, and
    /// whether more follow. Each is checked against the members; one that
    /// does not check out is an error, since what follows it cannot be
    /// taken in order.
    pub async fn ledger_read(
        &self,
        town: &TownName,
        after: u64,
    ) -> Result<(Vec<(u64, crate::ledger::Entry, Signed)>, bool), String> {
        self.read_ledger(town, Question::LedgerRead { town: town.clone(), after }).await
    }

    /// The complete project directory, without subscribing to its ledgers.
    pub async fn ledger_catalog(&self) -> Result<Vec<(TownName, u64)>, String> {
        match self.inner.ask(Question::LedgerCatalog).await? {
            Told::Towns { towns } => Ok(towns),
            _ => Err("the hub answered something other than the project directory".to_string()),
        }
    }

    /// Reads a town for an explicit user query, checking the same signatures
    /// as replication. Membership in the town is not a read boundary.
    pub async fn ledger_query(
        &self,
        town: &TownName,
        after: u64,
    ) -> Result<(Vec<(u64, crate::ledger::Entry, Signed)>, bool), String> {
        self.read_ledger(town, Question::LedgerQuery { town: town.clone(), after }).await
    }

    async fn read_ledger(
        &self,
        town: &TownName,
        question: Question,
    ) -> Result<(Vec<(u64, crate::ledger::Entry, Signed)>, bool), String> {
        let answer = self.inner.ask(question).await?;
        self.read_ledger_answer(town, answer).await
    }

    async fn read_ledger_answer(
        &self,
        town: &TownName,
        answer: Told,
    ) -> Result<(Vec<(u64, crate::ledger::Entry, Signed)>, bool), String> {
        let (entries, more) = match answer {
            Told::Entries { entries, more } => (entries, more),
            _ => return Err("the hub answered something else".to_string()),
        };
        let mut table = self.inner.members().table.clone();
        // An entry by a machine that joined after the members were last
        // asked for: ask again, as for a call from it.
        let unknown = entries.iter().any(|(_, signed)| {
            signed
                .claim::<crate::ledger::Entry>()
                .is_ok_and(|entry| table.get(&entry.author).is_none() && table.revoked(&entry.author).is_none())
        });
        if unknown {
            self.inner.refresh().await?;
            table = self.inner.members().table.clone();
        }
        let mut read = Vec::new();
        for (seq, signed) in entries {
            let entry = crate::ledger::open(&signed, &self.inner.membership.world, &table)
                .map_err(|error| format!("entry {seq} of {town}: {error}"))?;
            if entry.town != *town {
                return Err(format!("entry {seq} of {town} is for the town {}", entry.town));
            }
            read.push((seq, entry, signed));
        }
        Ok((read, more))
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
    /// given; `scope` what it may do, when not everything.
    pub async fn invite(
        &self,
        name: Option<MachineName>,
        expires: Timestamp,
        kind: Kind,
        scope: Option<Scope>,
    ) -> Result<Code, String> {
        self.refresh().await?;
        self.inner.members().table.may_invite(&self.inner.membership.name, scope.as_ref().unwrap_or(&Scope::all()))?;
        let invite = SecretKey::generate().map_err(|error| error.to_string())?;
        let membership = &self.inner.membership;
        let endorsement = Endorsement {
            world: membership.world.clone(),
            inviter: membership.name.clone(),
            invite: invite.public(),
            expires,
            name,
            kind,
            scope,
            at: Some(Timestamp::now()),
        };
        let endorsement = Signed::new(&self.inner.key, Domain::Endorsement, &endorsement);
        self.inner.ask(Question::Invite { endorsement: endorsement.clone() }).await?;
        Ok(Code::Invitation {
            hub: membership.hub.clone(),
            cert: membership.cert,
            world: membership.world.clone(),
            founder: membership.founder,
            endorsement,
            seed: crate::sign::Secret(invite.seed()),
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
            at: Timestamp::now(),
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
                at: Timestamp::now(),
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

    fn connection(&self) -> Option<Connection> {
        self.connection.lock().expect("never poisoned").clone()
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

    async fn key_of(&self, name: &MachineName) -> Option<PublicKey> {
        self.member(name).await.map(|member| member.key)
    }

    async fn refresh(&self) -> Result<(), String> {
        let _one = self.refreshing.lock().await;
        let Told::Members { records, revocations, grants, presence } = self.ask(Question::Members).await? else {
            return Err("the hub answered something else".to_string());
        };
        let mut members = self.members();
        let (table, known) = trust::check(
            &self.membership.world,
            &self.membership.founder,
            &records,
            &revocations,
            &grants,
            &members.file.known,
        );
        let changed = known != members.file.known
            || records != members.file.records
            || revocations != members.file.revocations
            || grants != members.file.grants;
        if changed {
            let refused: Vec<String> = table.refused.iter().map(|(what, why)| format!("{what}: {why}")).collect();
            if !refused.is_empty() {
                self.node.log(format!("refused from the hub's list of members: {}", refused.join("; ")));
            }
            members.file = MembersFile { known, records, revocations, grants };
            state::write(&self.dir.members(), &members.file)
                .map_err(|error| format!("keeping the members: {error}"))?;
        }
        for (name, _) in table.revoked_members() {
            self.sessions.drop_member(name);
        }
        members.table = table;
        members.presence = presence;
        Ok(())
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
            let ours = members
                .file
                .records
                .iter()
                .filter_map(|signed| signed.claim::<Record>().ok())
                .find(|record| record.name == *me && record.key == self.key.public())
                .ok_or_else(|| format!("the hub's list has no record of {me} with this machine's key"))?;
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
