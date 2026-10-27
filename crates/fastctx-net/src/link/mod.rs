//! A machine's link to one hub, held by its node.
//!
//! The link keeps one control connection to the hub open for as long as the
//! node runs, connecting again whenever it drops: by its own line, and when
//! that is down through a machine that allows direct connections and
//! forwards the hub's bytes. Over it the node calls other machines and
//! answers their calls, sends letters and takes the letters kept for it,
//! and asks the hub for its view of the directory and to make changes for
//! an account. With the machines that allow direct connections it keeps
//! direct links besides (`direct`), and calls between the two go over those
//! while they are up. Everything another machine says is checked here
//! against the view before the node sees it, whichever way it came: that it
//! is for this machine, and that the directory lets the sender say it for
//! the account it names (`crate::directory`). The node checks again what
//! each call asks.

pub mod account_records;
mod accounts;
mod addresses;
mod afoot;
mod answering;
mod answers;
mod calls;
mod changes;
mod clock;
mod collab;
mod direct;
mod kernel;
mod ledger;
mod letters;
mod posting;
pub use posting::{Posted, Posting};
mod line;
mod login_identity;
mod machine_log;
mod memory;
mod moving;
mod outgoing;
mod plans;
mod presence;
mod profile;
mod questions;
mod session;
pub mod state;
mod update_policy;
pub mod versions;
mod ways;

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use fastctx_types::access::{Purpose, TownId};
use fastctx_types::contract::Pool;
use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use tokio::sync::{Notify, mpsc, oneshot, watch};

use crate::data::{Endpoint, Stream};
use crate::dial::Upstream;
use crate::directory::View;
use crate::envelope::{Envelope, Kind};
use crate::message::LetterId;
use crate::sign::{Domain, PublicKey, SecretKey, Signed};
use crate::wire::{CallStatus, Change, Question, Told};
pub use afoot::Elsewhere;
pub use direct::DataWay;
pub use ledger::{Page, PageItem};
use state::{Counter, LettersFile};
pub use state::{Membership, NetDir};
pub use ways::Way;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What a caller does with its end of a call's data connection, the source.
pub type Feed = Box<dyn FnOnce(Stream) -> BoxFuture<Result<(), String>> + Send>;

/// Who says something to this machine: the member it comes from, and the
/// account it speaks for there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// The member, by its name now.
    pub machine: MachineName,
    pub account: AccountName,
    /// The admitted computer's immutable public identity.
    pub key: PublicKey,
    /// Its known names, retained only for older stored records.
    pub names: Vec<MachineName>,
    /// The original envelope name, used only to admit older message bodies.
    pub sent_as: MachineName,
}

/// What the link hands the node: calls to answer and letters to keep.
pub trait Node: Send + Sync + 'static {
    /// Known local login directories, without scanning or asking a host.
    fn login_homes(&self, agent: &fastctx_types::AgentName) -> Result<Vec<(std::path::PathBuf, bool)>, String> {
        Err(format!("this device has no local {agent} logins"))
    }
    /// Takes the place a call from `from` takes here while it is answered,
    /// of the kind `pool` (`fastctx_types::contract::Pool`), before anything
    /// waits on it; or says why there is none: this machine is answering as
    /// many of that kind as it takes. The place is let go when the guard is
    /// dropped.
    fn place(
        &self,
        machine: &MachineName,
        account: &AccountName,
        pool: Pool,
    ) -> Result<Box<dyn Send>, fastctx_types::ipc::Failure> {
        let _ = (machine, account, pool);
        Ok(Box::new(()))
    }
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
    /// An account record changed; consumers fetch the authoritative body as needed.
    fn account_record_changed(&self, _change: &fastctx_types::account_records::Changed) {}
    /// The hub calls this machine `to` now, no longer `from`.
    fn renamed(&self, from: &MachineName, to: &MachineName) {
        let _ = (from, to);
    }
    /// The link connected to the hub again and has asked it for the view.
    fn connected(&self) {}
    /// The hub ended this machine's place on it, for good: the machine was
    /// taken off, or, `dissolved`, the whole network was; `then`, what its
    /// remover asked it to do now.
    fn place_ended(&self, dissolved: bool, then: Option<fastctx_types::uninstall::Then>) {
        let _ = (dissolved, then);
    }
    /// The ways to other machines may have changed (`Link::way_to`): the
    /// line to the hub came or went, the hub said anew who is online, or a
    /// direct link came up or went down.
    fn ways_changed(&self) {}
    /// What the hub told of what is afoot on the other machines changed
    /// (`Link::afoot`), or the session on now has given its first telling
    /// of it (`Link::afoot_ready`).
    fn afoot_changed(&self) {}
    /// Deletes what this member keeps of the conversation the hub has it
    /// forget, and says how that went (`crate::wire::Down::Forget`). The hub
    /// tells it again at the next session until it says it is done, so it
    /// may come for what is gone already.
    fn forget(&self, forgetting: crate::wire::Forgetting) -> BoxFuture<crate::wire::Forgot>;
    /// What the hub tells of the network's versions changed
    /// (`Link::versions`).
    fn versions_changed(&self) {}
    /// Retains a local package throughout preparation and delivery to a hub.
    fn retain_package(&self, version: &str) -> Box<dyn Send + Sync> {
        let _ = version;
        Box::new(())
    }
    /// Brings here the package the hub asks for, for machines that cannot
    /// reach GitHub, checked against its signature, and says where it is
    /// (`crate::wire::Down::Fetch`).
    fn relay(
        &self,
        fetch: fastctx_types::update::Fetch,
        progress: tokio::sync::watch::Sender<fastctx_types::update::PackageProgress>,
    ) -> BoxFuture<Result<std::path::PathBuf, String>> {
        let _ = (fetch, progress);
        Box::pin(async { Err("this device does not fetch packages for others".to_string()) })
    }
    /// Lets other computers reach this machine at exactly the TCP `ports`
    /// through its own firewall, as far as the World does that, and says
    /// what it knows of that firewall: this machine takes direct connections
    /// there (`crate::peer`). With no ports, what was let in before is
    /// closed again. It may block.
    fn open_ports(&self, ports: &BTreeSet<u16>) -> Result<crate::wire::Firewall, String> {
        let _ = ports;
        Ok(crate::wire::Firewall::Unknown { why: "this device does not look at its firewall".to_string() })
    }
    fn log(&self, line: String);
}

/// How the link stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not connected; trying. `last_error` says why the last try failed.
    Connecting { since: Timestamp, last_error: Option<String> },
    /// Connected at `address`, to the hub or through the machine `through`
    /// that forwards to it; `route` says the way the connection leaves this
    /// machine.
    Online { since: Timestamp, address: SocketAddr, through: Option<String>, route: fastctx_types::world::Route },
    /// The hub removed this machine, or the link was stopped locally.
    /// Other refusals remain retryable.
    Stopped { reason: String },
}

/// Why a call got no answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CallFailure {
    #[error("this machine is not connected to the hub now ({reason}); nothing was sent")]
    NotConnected { reason: String },
    /// The machines are not listed: a session sees only those it may reach
    /// (`world-network.md`, 一个居民替谁干活), which `fastctx nodes` lists.
    #[error("there is no machine named {name}; `fastctx nodes` lists those within reach")]
    NotMember { name: MachineName },
    #[error("{name} is offline{}", last_seen.map(offline_for).unwrap_or_default())]
    Offline { name: MachineName, last_seen: Option<Timestamp> },
    #[error("the hub did not pass the call to {name}: {reason}; nothing ran there")]
    Refused { name: MachineName, reason: String },
    #[error("the call to {name} was not made: {reason}; nothing ran there")]
    NotAllowed { name: MachineName, reason: String },
    #[error("{name} did not answer ({reason}); whether it did what was asked is not known")]
    Unknown { name: MachineName, reason: String },
    /// It answered, in a form this release cannot read: a newer release's.
    #[error(
        "{name} answered in a form this Fastctx cannot read ({reason}): it may run a newer release; whether it did what was asked is not known"
    )]
    Unreadable { name: MachineName, reason: String },
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

#[derive(Clone)]
pub struct Link {
    inner: Arc<Inner>,
}

/// What a call went out on, so the end of that connection ends only the
/// calls on it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CallOn {
    /// The control connection to the hub of this generation.
    Hub(u64),
    /// The direct link with that machine, of this generation.
    Peer(MachineName, u64),
}

/// Where a call another machine made is answered.
pub(crate) enum Replier {
    /// Through the hub, which numbered the call.
    Hub(outgoing::Outgoing, u64),
    /// Over the direct link it came on, numbered by its caller.
    Peer(Arc<direct::PeerConn>, u64),
}

pub(crate) struct Inner {
    /// This link itself, for what it starts that outlives a call.
    this: Weak<Inner>,
    dir: NetDir,
    /// The hub this link is to, the certificate pinned for it, and what
    /// kind of member this is there: none of them changes.
    hub: crate::ids::HubId,
    cert: crate::ids::CertPin,
    kind: crate::directory::Kind,
    /// This machine's place on the hub as its membership file keeps it: its
    /// name there changes when the hub renames it (`Inner::follow_name`).
    membership: Mutex<Membership>,
    /// Where the hub is reached now (`addresses`): the membership's, until
    /// a hub that moved says otherwise.
    addresses: Mutex<Vec<SocketAddr>>,
    key: SecretKey,
    node: Arc<dyn Node>,
    counter: Counter,
    letters: Mutex<Letters>,
    /// Replaced whole on each refresh, so a reader holds it as it was.
    view: Mutex<Arc<View>>,
    /// One refresh of the view at a time, or one change to it; others wait,
    /// and one that waited takes what a refresh begun meanwhile found.
    refreshing: tokio::sync::Mutex<Refreshed>,
    /// How many refreshes of the view began.
    refreshes: AtomicU64,
    status: watch::Sender<Status>,
    record_changes: tokio::sync::broadcast::Sender<Option<fastctx_types::account_records::Changed>>,
    connection: Mutex<Option<Connection>>,
    calls: Mutex<HashMap<u64, Waiter>>,
    /// The calls of other machines this one is answering (`answers`).
    answers: answers::Answers,
    asks: questions::Questions,
    next_id: AtomicU64,
    /// Woken when a letter is put in the outbox.
    outbox_changed: Notify,
    /// Held while one of the hub's letters is taken (`session::take_letters`).
    taking: fastctx_platform::doing::Mutex<()>,
    /// What this session was last told of how each machine is connected,
    /// and when among the hub's changes (`presence`).
    presences: Mutex<HashMap<MachineName, (u64, crate::directory::Presence)>>,
    /// The hub's clock beside this machine's (`clock`).
    clock: Mutex<clock::Clock>,
    /// An application knows when its network changed or it returned to the foreground.
    reconnect: Notify,
    /// The session on now is to be asked at once whether the hub still
    /// hears it (`session::run`).
    check: Notify,
    /// How many of those checks the hub answered.
    checks: watch::Sender<u64>,
    /// Woken when the ways to the hub this machine should connect by changed.
    rerouted: Notify,
    /// The way to the hub that last opened a session: tried first next time,
    /// so a machine whose own line is closed does not wait it out on every
    /// return (`session::connect`).
    last_way: Mutex<Option<Upstream>>,
    /// Counts the changes of this machine's network the link noticed; the
    /// direct links dial again at once on each.
    network: watch::Sender<u64>,
    /// The round trip to the hub, as last measured.
    hub_ms: Mutex<Option<u32>>,
    /// How the line to the hub has held (`line`).
    line: Mutex<line::Line>,
    /// The direct links (`direct`).
    direct: direct::Direct,
    /// What is afoot here and elsewhere (`afoot`).
    afoots: afoot::Afoots,
    /// The versions told and heard (`versions`).
    versions: versions::Versions,
    /// The task that keeps the connection.
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The hub said, refusing this machine, that its network was dissolved.
    dissolved: std::sync::atomic::AtomicBool,
    /// The hub said, refusing this machine, what it is to do now.
    then: Mutex<Option<fastctx_types::uninstall::Then>>,
}

struct Letters {
    file: LettersFile,
    /// The number the next letter gets.
    next: u64,
}

/// What the last refresh of the view found, by its number.
#[derive(Default)]
pub(crate) struct Refreshed {
    last: Option<(u64, Result<(), String>)>,
}

/// The current connection: where to put frames to send on it, and the way
/// to the hub it is on, which data connections take first.
#[derive(Clone)]
pub(crate) struct Connection {
    generation: u64,
    closed: watch::Receiver<Option<String>>,
    up: outgoing::Outgoing,
    upstream: Upstream,
    /// The address it was dialed at: one of the hub's, or a forwarder's.
    address: SocketAddr,
    /// The ways this machine tried, through the one it is on.
    tried: Vec<Upstream>,
}

/// A call waiting for what answers it.
pub(crate) struct Waiter {
    events: mpsc::UnboundedSender<CallEvent>,
    on: CallOn,
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
        // Noticing this machine's network change or wake only hastens a
        // reconnect: without it, heartbeats and retries still find the hub.
        let changes = match changes::Changes::new(node.clone()) {
            Ok(changes) => Some(changes),
            Err(error) => {
                node.log(format!("not watching this machine's network and sleep: {error}"));
                None
            }
        };
        Self::start_with_changes(dir, membership, key, node, changes)
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
        let outbox = letters::Outbox::new(&dir);
        let numbers = outbox.numbers()?;
        // What the hub held when a stop came between keeping that and
        // deleting the letters is deleted now.
        for seq in numbers.iter().filter(|seq| **seq <= letters_file.held) {
            outbox.remove(*seq)?;
        }
        let highest = numbers.last().copied().unwrap_or(0);
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
        let policy = update_policy::UpdatePolicy::read(dir.path())?;
        let (status, _) = watch::channel(Status::Connecting { since: Timestamp::now(), last_error: None });
        let receiving = dir.path().display().to_string();
        let inner = Arc::new_cyclic(|this| Inner {
            this: this.clone(),
            dir,
            addresses: Mutex::new(membership.addresses.clone()),
            hub: membership.id.clone(),
            cert: membership.cert,
            kind: membership.kind,
            membership: Mutex::new(membership),
            key,
            node,
            counter,
            letters: Mutex::new(Letters { file: letters_file, next }),
            view: Mutex::new(Arc::new(view)),
            refreshing: tokio::sync::Mutex::new(Refreshed::default()),
            refreshes: AtomicU64::new(0),
            status,
            record_changes: tokio::sync::broadcast::channel(1024).0,
            connection: Mutex::new(None),
            calls: Mutex::new(HashMap::new()),
            answers: answers::Answers::default(),
            asks: questions::Questions::default(),
            next_id: AtomicU64::new(0),
            dissolved: std::sync::atomic::AtomicBool::new(false),
            then: Mutex::new(None),
            outbox_changed: Notify::new(),
            taking: fastctx_platform::doing::Mutex::new(
                fastctx_platform::doing::Rank::RECEIVING,
                "incoming letters",
                receiving,
                (),
            ),
            presences: Mutex::new(HashMap::new()),
            clock: Mutex::default(),
            reconnect: Notify::new(),
            check: Notify::new(),
            checks: watch::channel(0).0,
            rerouted: Notify::new(),
            last_way: Mutex::new(None),
            network: watch::channel(0).0,
            hub_ms: Mutex::new(None),
            line: Mutex::default(),
            direct: direct::Direct::default(),
            afoots: afoot::Afoots::default(),
            versions: versions::Versions::with_policy(policy),
            task: Mutex::new(None),
        });
        inner.line_started();
        // A machine that allows direct connections takes them from the
        // roster it last had, before it reaches the hub, and whether or not
        // it does.
        inner.follow_view();
        let task = tokio::spawn(session::run(inner.clone(), changes));
        *inner.task.lock().expect("never poisoned") = Some(task);
        Ok(Self { inner })
    }

    /// The application's network changed or it came back to the front: a
    /// session that is on is asked at once whether the hub still hears it,
    /// and kept when it does (`checks` counts the answers); otherwise this
    /// machine dials again at once, skipping the retry delay. A stopped or
    /// removed membership is never restarted by this signal.
    pub fn reconnect(&self) {
        self.inner.reconnect.notify_one();
    }

    /// How many checks the hub has answered, as it changes. One asked after
    /// `reconnect` and answered means the session on held; one unanswered
    /// ends the session, and `subscribe` says so.
    pub fn checks(&self) -> watch::Receiver<u64> {
        self.inner.checks.subscribe()
    }

    /// Where this membership is kept.
    pub fn dir(&self) -> &std::path::Path {
        self.inner.dir.path()
    }

    /// Whether the hub said, ending this machine's place, that the whole
    /// network was dissolved.
    pub fn dissolved(&self) -> bool {
        self.inner.dissolved.load(std::sync::atomic::Ordering::Relaxed)
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
        self.inner.direct.stop();
        *self.inner.connection.lock().expect("never poisoned") = None;
        self.inner.status.send_replace(Status::Stopped { reason: "the link was stopped".to_string() });
        self.inner.ways_changed();
    }

    /// This machine's place on the hub. Its addresses are those it logged
    /// in with, or the hub gave since; where the hub is reached now is
    /// `hub_addresses`.
    pub fn membership(&self) -> Membership {
        self.inner.membership()
    }

    /// The hub this link is to.
    pub fn hub(&self) -> &crate::ids::HubId {
        &self.inner.hub
    }

    /// This machine's name on the hub now.
    pub fn name(&self) -> MachineName {
        self.inner.me()
    }

    /// Where the hub is reached now, in the order to try: what this machine
    /// logged in with, or, once the hub moved, what the hub said since.
    pub fn hub_addresses(&self) -> Vec<SocketAddr> {
        self.inner.hub_addresses()
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

    /// The view as last given, while this machine is a member of the hub:
    /// it stays while the line to the hub is down, and the machines in it
    /// are then not gone, only not heard from (`world-network.md`, 断开不是
    /// 结束). None once the hub removed this machine. What may be done on
    /// them is decided apart, on the view as it stands then.
    pub fn last_view(&self) -> Option<Arc<View>> {
        if matches!(self.status(), Status::Stopped { .. }) {
            return None;
        }
        let view = self.inner.view();
        view.me().is_some().then_some(view)
    }

    /// This machine's direct links with machines on this hub, as they stand.
    pub fn direct_links(&self) -> Vec<fastctx_types::accounts::LinkSeen> {
        self.inner.direct.seen()
    }

    /// Whether this machine listens for direct connections now, and why not
    /// when it should; `None` when it does not allow them.
    pub fn listening(&self) -> Option<Result<(), String>> {
        self.inner.direct.listening()
    }

    /// The round trip to the hub, as last measured.
    pub fn hub_ms(&self) -> Option<u32> {
        *self.inner.hub_ms.lock().expect("never poisoned")
    }

    /// Tells the hub that what is afoot on this machine is `afoot` now
    /// (`world-network.md`, 近况): the change from what it was told, as soon
    /// as a session to a hub that takes it is on, and all of it to each new
    /// session.
    pub fn tell_afoot(&self, afoot: fastctx_types::town::Afoot) {
        self.inner.tell_afoot(afoot);
    }

    /// What is afoot on each other machine of this hub as the hub last told
    /// it, as this machine may see it.
    pub fn afoot(&self) -> std::collections::BTreeMap<MachineName, Elsewhere> {
        self.inner.afoot_elsewhere()
    }

    /// Whether `afoot` is what the session on now first told, or there is
    /// no session; false between a session's start and its first telling.
    pub fn afoot_ready(&self) -> bool {
        self.inner.afoot_ready()
    }

    /// How this machine's calls to `machine` go now: over a direct link, or
    /// through the hub, and then why there is no direct link when it allows
    /// direct connections.
    pub fn path_to(&self, machine: &MachineName) -> fastctx_types::world::Path {
        if let Some(conn) = self.inner.direct.link(machine) {
            return fastctx_types::world::Path::Direct { ms: conn.ms() };
        }
        let allows = self.inner.view().member(machine).is_some_and(|member| !member.direct.is_empty());
        let why = allows
            .then(|| self.inner.direct.seen().into_iter().find(|link| link.peer == *machine).and_then(|link| link.why))
            .flatten();
        fastctx_types::world::Path::Hub { why }
    }

    /// The hub's short name, as the hub last said it.
    pub fn hub_name(&self) -> fastctx_types::HubName {
        self.inner.view().hub_name.clone()
    }

    /// Asks the hub for this machine's view again.
    pub async fn refresh(&self) -> Result<(), String> {
        self.inner.refresh().await
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
        // A call's future is tens of kilobytes; on the heap it does not grow
        // every future that makes a call, which a build without
        // optimizations keeps whole on the stack, once for each. Its type is
        // erased too: proving a caller's future `Send` then stops here instead
        // of descending through every layer to the TLS stream, which passes
        // the compiler's recursion limit where a device calls a hub itself.
        let within = call.contract().within;
        let calling: Pin<Box<dyn Future<Output = Result<RemoteResult, CallFailure>> + Send + '_>> = Box::pin(async {
            let call = match Box::pin(self.login_call(to, account, call)).await {
                Ok(call) => call,
                Err(reason) => return Ok(Err(fastctx_types::ipc::Failure::not_run(reason))),
            };
            Box::pin(self.inner.call(to, account, purpose, call, None)).await
        });
        match within {
            Some(within) => {
                fastctx_platform::doing::outside(format!("{to}'s call including preparation"), within, calling)
                    .await
                    .map_err(|late| CallFailure::Unknown { name: to.clone(), reason: late.to_string() })?
            }
            None => calling.await,
        }
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
        // Boxed and erased as in `call`.
        let within = call.contract().within;
        let calling: Pin<Box<dyn Future<Output = Result<RemoteResult, CallFailure>> + Send + '_>> = Box::pin(async {
            let call = match Box::pin(self.login_call(to, account, call)).await {
                Ok(call) => call,
                Err(reason) => return Ok(Err(fastctx_types::ipc::Failure::not_run(reason))),
            };
            Box::pin(self.inner.call(to, account, purpose, call, Some(feed))).await
        });
        match within {
            Some(within) => {
                fastctx_platform::doing::outside(format!("{to}'s call including preparation"), within, calling)
                    .await
                    .map_err(|late| CallFailure::Unknown { name: to.clone(), reason: late.to_string() })?
            }
            None => calling.await,
        }
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
        self.post_letter(to, account, purpose, id, letter).map(|_| ())
    }

    /// Posts a letter and returns the durable reference used to observe hub custody.
    pub fn post_letter(
        &self,
        to: &MachineName,
        account: &AccountName,
        purpose: Purpose,
        id: LetterId,
        letter: Letter,
    ) -> io::Result<Posted> {
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

    /// Acknowledges a project deletion after this member finished its local work.
    pub async fn project_deleted(&self, town: &fastctx_types::access::TownId) -> Result<(), String> {
        match self.inner.ask(Question::ProjectDeleted { town: town.clone() }).await? {
            Told::Done => Ok(()),
            _ => Err("the hub did not acknowledge the project deletion".into()),
        }
    }

    /// The account of this member's logins the conversation of `resident`
    /// on `machine` works for, as the hub last told what is afoot there.
    pub fn account_of(
        &self,
        machine: &MachineName,
        resident: &fastctx_types::ResidentId,
    ) -> Result<AccountName, String> {
        use fastctx_types::access::Acting;
        let view = self.view();
        let mine = view.me().map(|me| me.logins.clone()).unwrap_or_default();
        let afoot = self.afoot();
        let told = afoot
            .iter()
            .find(|(name, _)| view.directory.same(name, machine))
            .and_then(|(_, told)| told.seen.residents.get(resident))
            .ok_or_else(|| format!("{resident} on {machine} is not one this member is told of"))?;
        let theirs: Vec<AccountName> = match &told.about.acting {
            // One opened on its machine works for every login there.
            Acting::Here => {
                view.member(machine).map(|member| member.logins.iter().cloned().collect()).unwrap_or_default()
            }
            Acting::Accounts { accounts } => accounts
                .iter()
                .filter(|account| account.hub == self.membership().id)
                .map(|account| account.account.clone())
                .collect(),
        };
        theirs
            .into_iter()
            .find(|account| mine.contains(account))
            .ok_or_else(|| format!("this member is logged in to no account {resident} works for"))
    }

    /// Asks the hub to delete the conversation at `conversation`, which
    /// works for `account`, from every member keeping something of it
    /// (`Question::Forget`). `Ok(false)`: it could not be asked now, not
    /// being connected, or the connection ended before the hub answered,
    /// which leaves whether it took it unknown; asking again is safe, as the
    /// hub takes one deletion of a conversation for an account once.
    pub async fn forget(&self, account: &AccountName, conversation: &fastctx_types::Address) -> Result<bool, String> {
        let Some(connection) = self.inner.connection() else { return Ok(false) };
        if !self.view().forgets {
            return Err("the hub runs an earlier Fastctx, which does not delete conversations; update it first".into());
        }
        let question = Question::Forget { account: account.clone(), conversation: conversation.clone() };
        match self.inner.ask_on(connection, question).await? {
            Some(Told::Done) => Ok(true),
            Some(other) => Err(format!("the hub answered something other than taking the deletion: {other:?}")),
            None => Ok(false),
        }
    }
}

impl Inner {
    fn view(&self) -> Arc<View> {
        self.view.lock().expect("never poisoned").clone()
    }

    /// This machine's place on the hub, as kept now.
    pub(crate) fn membership(&self) -> Membership {
        self.membership.lock().expect("never poisoned").clone()
    }

    /// This machine's name on the hub now.
    pub(crate) fn me(&self) -> MachineName {
        self.membership.lock().expect("never poisoned").name.clone()
    }

    /// Changes the membership as `change` does and keeps it in the
    /// membership file. The change holds while the node runs even when
    /// keeping it fails; after a restart the hub says it again.
    pub(crate) fn keep_membership(&self, change: impl FnOnce(&mut Membership)) -> io::Result<()> {
        let mut membership = self.membership.lock().expect("never poisoned");
        change(&mut membership);
        state::write(&self.dir.membership(), &*membership)
    }

    /// Takes `name` as this machine's name on the hub, which renamed it
    /// (`world-network.md`, 机器的名字): what it is called there is the hub's
    /// to say, and the hub says it of this machine's key. Kept in the
    /// membership file, and told to the node.
    pub(crate) fn follow_name(&self, name: MachineName) {
        let before = self.me();
        if before == name {
            return;
        }
        match self.keep_membership(|kept| kept.name = name.clone()) {
            Ok(()) => self.node.log(format!("the hub calls this machine {name} now, no longer {before}")),
            Err(error) => {
                self.node.log(format!("the hub calls this machine {name} now; recording that failed: {error}"))
            }
        }
        self.node.renamed(&before, &name);
    }

    fn connection(&self) -> Option<Connection> {
        self.connection.lock().expect("never poisoned").clone()
    }

    /// How this machine's lines go, for the hub.
    fn paths(&self) -> crate::directory::Paths {
        crate::directory::Paths { hub_ms: *self.hub_ms.lock().expect("never poisoned"), links: self.direct.seen() }
    }

    /// The ways to the hub, in order: this machine's own line, then through
    /// each machine that forwards for it, in the hub's order. Before its
    /// first view, a machine that joined through relays reaches the hub
    /// through those its code named.
    fn upstreams(&self) -> Vec<Upstream> {
        let view = self.view();
        let forwarders = view.forwarders.iter().filter(|forwarder| !view.is_me(&forwarder.name)).cloned();
        let mut ways: Vec<Upstream> =
            std::iter::once(Upstream::Hub(self.hub_addresses())).chain(forwarders.map(Upstream::Through)).collect();
        if view.me().is_none() {
            ways.extend(self.membership().relays.into_iter().map(|relay| {
                Upstream::Through(crate::dial::Forwarder { name: crate::dial::relay_name(), at: vec![relay] })
            }));
        }
        ways
    }

    /// Where a data connection through the hub goes: the way the control
    /// connection is on, then every other.
    fn data_endpoint(&self) -> Endpoint {
        let mut upstreams = Vec::new();
        if let Some(connection) = self.connection() {
            upstreams.push(connection.upstream);
        }
        for upstream in self.upstreams() {
            if !upstreams.contains(&upstream) {
                upstreams.push(upstream);
            }
        }
        Endpoint::Hub { upstreams, hub: self.hub.clone(), cert: self.cert }
    }

    /// Hands `event` to the call `id` made on `on`.
    fn call_event(&self, on: &CallOn, id: u64, event: CallEvent) {
        if let Some(waiter) = self.calls.lock().expect("never poisoned").get(&id).filter(|waiter| waiter.on == *on) {
            let _ = waiter.events.send(event);
        }
    }

    /// Tells every call made on `on` that it ended, and why.
    fn fail_calls_on(&self, on: &CallOn, reason: &str) {
        for waiter in self.calls.lock().expect("never poisoned").values().filter(|waiter| waiter.on == *on) {
            let _ = waiter.events.send(CallEvent::Disconnected(reason.to_string()));
        }
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
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        let _wait = fastctx_platform::doing::waiting_outside(
            "the first connection to the hub",
            std::time::Duration::from_secs(60),
        );
        let mut status = self.status.subscribe();
        loop {
            if let Some(connection) = self.connection() {
                return Some(connection);
            }
            if !matches!(*status.borrow_and_update(), Status::Connecting { last_error: None, .. }) {
                return self.connection();
            }
            if !matches!(tokio::time::timeout_at(deadline, status.changed()).await, Ok(Ok(()))) {
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

    /// A call to `name`, which is no member, refused.
    fn not_member(&self, name: &MachineName) -> CallFailure {
        CallFailure::NotMember { name: name.clone() }
    }

    /// Asks the hub for the view again. Of the many that may ask at once, as
    /// calls from a machine the view does not know yet do, those that waited
    /// for one begun after they asked take what it found, so that a hub slow
    /// to answer is waited for once, not once each.
    async fn refresh(&self) -> Result<(), String> {
        let asked = self.refreshes.load(Ordering::SeqCst);
        let mut one = self.refreshing.lock().await;
        if let Some((began, found)) = &one.last
            && *began > asked
        {
            return found.clone();
        }
        let began = self.refreshes.fetch_add(1, Ordering::SeqCst) + 1;
        let found = self.refresh_now().await;
        one.last = Some((began, found.clone()));
        found
    }

    async fn refresh_now(&self) -> Result<(), String> {
        let Told::Directory { mut view } = self.ask(Question::Directory).await? else {
            return Err("the hub answered something else".to_string());
        };
        // Its times by this machine's clock, and who came or went since the
        // hub made it.
        self.localize(&mut view);
        self.with_presences(&mut view);
        if view.hub != self.hub {
            return Err(format!("the hub gave a view of {}, not of {}", view.hub, self.hub));
        }
        let me = self.me();
        if view.me != me {
            // The hub renamed this machine: it says so of this machine's key.
            let renamed = view.me().is_some_and(|member| member.key == self.key.public() && member.named(&me));
            if !renamed {
                return Err(format!("the hub gave the view of {}, not of {me}", view.me));
            }
            self.follow_name(view.me.clone());
        }
        let changed = *self.view() != *view;
        if changed {
            state::write(&self.dir.view(), &*view).map_err(|error| format!("keeping the view: {error}"))?;
            self.follow_addresses(&view);
            let mut current = self.view.lock().expect("never poisoned");
            // Who came or went while it was being kept goes in with it: a
            // change told meanwhile went into the view it replaces.
            self.with_presences(&mut view);
            *current = Arc::from(view);
            drop(current);
            if let Some(this) = self.this.upgrade() {
                this.follow_view();
            }
            self.prune_afoot();
            self.ways_changed();
        }
        self.reroute_if_changed();
        Ok(())
    }

    /// Checks that `envelope` is for this machine and that the view lets
    /// its sender say it, for the account it names, asking the hub for the
    /// view again once when it does not: a grant the hub just made may not
    /// be in the view yet.
    async fn admit(&self, envelope: &Envelope) -> Result<Caller, String> {
        match self.admits_now(envelope) {
            Ok(caller) => Ok(caller),
            Err(_) => {
                if let Err(error) = self.refresh().await {
                    self.node.log(format!("asking the hub for the view: {error}"));
                }
                self.admits_now(envelope)
            }
        }
    }

    /// Who said `envelope`, by the view as it is now: why not, when it does
    /// not let them, which a grant made just now may not be in yet.
    pub(super) fn admits_now(&self, envelope: &Envelope) -> Result<Caller, String> {
        if envelope.hub != self.hub {
            return Err(format!("it is for {} on {}", envelope.to, envelope.hub));
        }
        let (account, purpose) = envelope.standing()?;
        let view = self.view();
        if !view.is_me(&envelope.to) {
            return Err(format!("it is for {} on {}", envelope.to, envelope.hub));
        }
        // A name this machine or the sender had before the hub renamed it
        // names it all the same, as in a letter kept since; the sender is
        // known by its name now.
        match (envelope.kind, purpose) {
            (Kind::Letter, Purpose::Echo) => view.admits_echo(&envelope.from, account, &view.me),
            _ => view.admits(&envelope.from, account, &view.me, purpose),
        }?;
        let member = view.member(&envelope.from).ok_or("the admitted sender has no directory identity")?;
        let machine = member.name.clone();
        let mut names = member.was.clone();
        names.push(machine.clone());
        names.push(envelope.from.clone());
        names.sort();
        names.dedup();
        Ok(Caller { machine, account: account.clone(), key: member.key, names, sent_as: envelope.from.clone() })
    }

    async fn ask(&self, question: Question) -> Result<Told, String> {
        fastctx_platform::doing::outside("the hub's answer, including its first connection", within(&question), async {
            let connection =
                self.connected().await.ok_or_else(|| format!("not connected to the hub: {}", self.not_connected()))?;
            self.ask_on(connection, question)
                .await?
                .ok_or_else(|| "the connection to the hub ended before it answered".to_string())
        })
        .await
        .map_err(|late| late.to_string())?
    }

    /// None means that the connection ended, distinct from a hub refusal. A
    /// hub that does not answer in time is said to (`within`): a caller may
    /// hold a lock over many conversations while it asks.
    async fn ask_on(&self, connection: Connection, question: Question) -> Result<Option<Told>, String> {
        let within = within(&question);
        let id = self.next_id();
        let frames = crate::question_parts::frames(id, question, self.view().ledger_parts)?;
        let multipart = frames.len() > 1;
        let (answer, answered) = oneshot::channel();
        self.asks.insert(id, connection.generation, answer, None);
        struct Waiting<'a>(&'a Inner, u64, Option<outgoing::Outgoing>);
        impl Drop for Waiting<'_> {
            fn drop(&mut self) {
                self.0.asks.remove(self.1);
                if let Some(up) = &self.2 {
                    let _ = up.send(crate::wire::Up::CancelQuestion { id: self.1 });
                }
            }
        }
        let _waiting = Waiting(self, id, multipart.then(|| connection.up.clone()));
        for frame in frames {
            if connection.up.send(frame).is_err() {
                return Ok(None);
            }
        }
        // The answer, or the end of the connection, which drops the sender.
        match fastctx_platform::doing::outside("the hub's answer", within, answered).await {
            Ok(Ok(answer)) => answer.map(Some),
            Ok(Err(_)) => Ok(None),
            Err(late) => Err(late.to_string()),
        }
    }

    fn letters(&self) -> MutexGuard<'_, Letters> {
        self.letters.lock().expect("never poisoned")
    }
}

/// How long the hub may take to answer `question`: a package it may fetch
/// from elsewhere and a move of the hub take minutes; everything else it
/// answers from what it holds.
pub(crate) fn within(question: &Question) -> std::time::Duration {
    std::time::Duration::from_secs(match question {
        Question::Package { .. } | Question::PrepareUpdate { .. } => 45 * 60,
        Question::HubMove { .. } => 10 * 60,
        _ => 60,
    })
}
