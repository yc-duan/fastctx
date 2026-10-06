//! A relay: the hub's forwarding layer, without its store (`world-network.md`,
//! 一个中枢，一棵树).
//!
//! A machine set to relay listens like the hub, with a certificate of its
//! own, and takes the sessions of the machines whose routes lead through it.
//! It carries them to its own upstream over one relay session
//! (`crate::attached`), where they count as connected; letters, questions and
//! the list of members stay at the root. What it can do itself it does
//! without the root: it answers heartbeats, passes a call between two
//! machines both below it, and pairs their data connections.
//!
//! A relay reads what the hub reads and no more: who speaks to whom, in
//! envelopes signed by the members that say them, around sealed bodies. It
//! checks what the hub checks before passing a call between two machines,
//! and takes nobody whose routes do not lead through it. When its upstream
//! is gone it lets every machine below it go as well, so none of them waits
//! on a relay that cannot carry what it says: they connect again, through
//! their fallback.

mod below;
mod calls;
mod data;
mod upward;

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use fastctx_net::sign::{Domain, PublicKey, Signature};
use fastctx_net::tls::HubCertificate;
use fastctx_net::tree::Endpoint;
use fastctx_net::trust::{Table, WorldId};
use fastctx_net::wire::{Down, Letters, Refusal, Up};
use fastctx_types::MachineName;
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;

use crate::Out;

/// What a relay needs from the machine it runs on.
pub trait Machine: Send + Sync + 'static {
    /// The members this machine trusts now.
    fn table(&self) -> Arc<Table>;
    /// The upstreams the relay connects through, in the order it tries
    /// them.
    fn upstreams(&self) -> Vec<Endpoint>;
    /// The machine's proof that it is on the connection whose TLS exporter
    /// is `binding`.
    fn prove(&self, binding: &[u8; 32]) -> Signature;
    /// Asks for the members again, as when a machine the relay does not
    /// know yet, or does not know to be routed through it, connects.
    fn refresh(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
    /// The relay carries sessions now, or no longer does.
    fn ready(&self, ready: bool);
    fn log(&self, line: String);
}

/// Where a relay listens, and the certificate machines below it pin.
pub struct Settings {
    pub listen: SocketAddr,
    pub certificate: HubCertificate,
}

/// A running relay; it stops when dropped, after writing upward what the
/// machines below it said before it stopped.
pub struct Relay {
    shared: Arc<Shared>,
    accepting: JoinHandle<()>,
}

impl Relay {
    /// Starts relaying as the member `me`, listening where `settings` says.
    pub async fn start(
        me: MachineName,
        world: WorldId,
        key: PublicKey,
        machine: Arc<dyn Machine>,
        settings: Settings,
    ) -> Result<Self, String> {
        let tls = tokio_rustls::TlsAcceptor::from(settings.certificate.server_config()?);
        let listen = settings.listen;
        let listener = TcpListener::bind(listen).await.map_err(|error| format!("listening on {listen}: {error}"))?;
        let shared = Arc::new(Shared {
            me,
            world,
            key,
            machine,
            state: Mutex::new(State::default()),
            next: AtomicU64::new(0),
            rerouted: Notify::new(),
            departures: Notify::new(),
            stopping: watch::channel(false).0,
        });
        shared.log(format!("relaying on {listen}; certificate {}", settings.certificate.pin()));
        let accepting = {
            let shared = shared.clone();
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((tcp, peer)) => {
                            fastctx_net::dial::configure(&tcp);
                            tokio::spawn(below::connection(shared.clone(), tls.clone(), tcp, peer));
                        }
                        // Out of descriptors, for one: the next accept may succeed.
                        Err(error) => {
                            shared.log(format!("accepting: {error}"));
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }
                }
            })
        };
        tokio::spawn(upward::run(shared.clone()));
        Ok(Self { shared, accepting })
    }

    /// The members changed: the relay connects again when the upstreams it
    /// should try first are no longer the ones it tried.
    pub fn members_changed(&self) {
        self.shared.members_changed();
    }

    /// Waits until this machine's own session is not attached here: it
    /// never was, or it ended and everything said on it was passed on.
    pub async fn released(&self) {
        loop {
            let departed = self.shared.departures.notified();
            tokio::pin!(departed);
            departed.as_mut().enable();
            if !self.shared.state().below.contains_key(&self.shared.me) {
                return;
            }
            departed.await;
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.accepting.abort();
        self.shared.stopping.send_replace(true);
        self.shared.lost(Refusal::Broken { reason: format!("{} stopped relaying", self.shared.me) });
        self.shared.machine.ready(false);
    }
}

pub(crate) struct Shared {
    me: MachineName,
    world: WorldId,
    key: PublicKey,
    machine: Arc<dyn Machine>,
    state: Mutex<State>,
    next: AtomicU64,
    /// Woken when the upstreams the relay should try changed.
    rerouted: Notify,
    /// Woken when a machine left below.
    departures: Notify,
    /// Set when the relay stops: the session upward writes what is queued
    /// and ends, and is not opened again.
    stopping: watch::Sender<bool>,
}

#[derive(Default)]
pub(crate) struct State {
    /// The relay session upward, while it is open.
    up: Option<Upward>,
    /// The machines attached below this relay, by name.
    below: HashMap<MachineName, Below>,
    /// The relay sessions of the relays right below this one.
    relays: HashMap<MachineName, Sub>,
    /// The calls this relay numbered, by its number.
    calls: HashMap<u64, calls::Call>,
    /// Data connections waiting here for their other end, by stream.
    waiting: HashMap<u64, data::Waiting>,
}

/// The relay session upward.
struct Upward {
    generation: u64,
    out: mpsc::UnboundedSender<Up>,
    endpoint: Endpoint,
    /// The upstreams the relay tried, through the one it is on.
    tried: Vec<Endpoint>,
}

/// A machine attached below.
pub(crate) struct Below {
    /// This relay's number for the attachment, which it gives upward.
    tag: u64,
    path: Path,
    /// The root welcomed it: until then nothing from beside it is passed to
    /// it here, for its first frame must be the welcome, and the root has
    /// not yet taken it.
    welcomed: bool,
}

impl Below {
    /// Whether what is for it may be passed here, between it and a machine
    /// beside it.
    pub(crate) fn here(&self) -> bool {
        self.welcomed
    }
}

enum Path {
    /// Connected to this relay.
    Here { out: mpsc::UnboundedSender<Out> },
    /// Attached through the relay `relay` right below, in its relay session
    /// `generation`, as that relay's attachment `tag`.
    Through { relay: MachineName, generation: u64, tag: u64, out: mpsc::UnboundedSender<Out> },
}

/// A relay session of a relay right below.
struct Sub {
    generation: u64,
    out: mpsc::UnboundedSender<Out>,
}

impl Below {
    /// Sends `out` to the machine `name` this is, through whatever it is
    /// attached by.
    fn send(&self, name: &MachineName, out: Out) -> bool {
        match &self.path {
            Path::Here { out: here } => here.send(out).is_ok(),
            Path::Through { tag, out: relay, .. } => {
                let frame = match out {
                    Out::Frame(frame) => frame,
                    Out::End(reason) => Down::Refused { reason },
                };
                relay.send(Out::Frame(Down::To { name: name.clone(), tag: *tag, frame: Box::new(frame) })).is_ok()
            }
        }
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("never poisoned")
    }

    fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn log(&self, line: String) {
        self.machine.log(line);
    }

    /// The member `key` belongs to, proved on this connection, when its
    /// routes lead through this relay. A machine routed here a moment ago
    /// may be told of before this relay has heard: it asks for the members
    /// once more before it refuses.
    async fn admit(&self, key: &PublicKey, proof: &Signature, binding: &[u8; 32]) -> Result<MachineName, Refusal> {
        if key.verify(Domain::Session, binding, proof).is_err() {
            return Err(Refusal::Broken { reason: "the proof of the machine's key does not verify".to_string() });
        }
        match self.admitted(key) {
            Err(_) => {
                self.machine.refresh().await;
                self.admitted(key)
            }
            admitted => admitted,
        }
    }

    fn admitted(&self, key: &PublicKey) -> Result<MachineName, Refusal> {
        let table = self.machine.table();
        let name = table.by_key(key).map(|member| member.name.clone()).ok_or(Refusal::NotMember)?;
        if !table.carries(&self.me, &name) {
            return Err(Refusal::NotAdmitted { reason: format!("{name}'s route does not lead through {}", self.me) });
        }
        Ok(name)
    }

    fn not_connected(&self) -> Refusal {
        Refusal::Broken { reason: format!("the relay {} is not connected to its upstream", self.me) }
    }

    /// Attaches `name` below, by `path`, in place of an earlier attachment
    /// of it, and tells the upstream. Returns the attachment's tag.
    fn attach(&self, name: &MachineName, path: Path, letters: Letters) -> Result<u64, Refusal> {
        let mut state = self.state();
        if state.up.is_none() {
            return Err(self.not_connected());
        }
        // A frame of a relay session below that ended as the upstream was
        // lost attaches nothing to what replaced it.
        if let Path::Through { relay, generation, .. } = &path
            && state.relays.get(relay).is_none_or(|sub| sub.generation != *generation)
        {
            return Err(Refusal::Broken { reason: format!("{relay}'s relay session here ended") });
        }
        let tag = self.next();
        if let Some(earlier) = state.below.insert(name.clone(), Below { tag, path, welcomed: false }) {
            earlier.send(name, Out::End(Refusal::Replaced));
            gone(&mut state, name, earlier.tag);
        }
        let up = state.up.as_ref().expect("checked above");
        let _ = up.out.send(Up::Attach { name: name.clone(), tag, letters });
        Ok(tag)
    }

    /// The attachment `tag` of `name` ended.
    fn left(&self, name: &MachineName, tag: u64) {
        let mut state = self.state();
        if state.below.get(name).is_some_and(|below| below.tag == tag) {
            state.below.remove(name);
            gone(&mut state, name, tag);
            drop(state);
            self.departed();
        }
    }

    fn departed(&self) {
        self.departures.notify_waiters();
    }

    /// Ends the attachment `tag` of `name`, saying why.
    fn end(&self, name: &MachineName, tag: u64, reason: Refusal) {
        let mut state = self.state();
        if let Some(below) = state.below.get(name).filter(|below| below.tag == tag) {
            below.send(name, Out::End(reason));
            state.below.remove(name);
            gone(&mut state, name, tag);
            drop(state);
            self.departed();
        }
    }

    /// Sends `frame` to `name` in its attachment `tag`, if that is still
    /// the one.
    fn tell(&self, name: &MachineName, tag: u64, frame: Down) {
        if let Some(below) = self.state().below.get(name).filter(|below| below.tag == tag) {
            below.send(name, Out::Frame(frame));
        }
    }

    /// Passes a frame of `name`'s upward.
    fn up_for(&self, name: &MachineName, tag: u64, frame: Up) {
        if let Some(up) = &self.state().up {
            let _ = up.out.send(Up::For { name: name.clone(), tag, frame: Box::new(frame) });
        }
    }

    /// The upstream is gone, or the relay stops: every machine below goes.
    fn lost(&self, reason: Refusal) {
        self.lost_to(reason.clone(), reason);
    }

    /// As `lost`, telling this machine's own session `mine` and the others
    /// `reason`: when the root revoked this machine, only its own session
    /// is told it is revoked.
    fn lost_to(&self, mine: Refusal, reason: Refusal) {
        let mut state = self.state();
        state.up = None;
        for (name, below) in state.below.drain() {
            let told = if name == self.me { mine.clone() } else { reason.clone() };
            below.send(&name, Out::End(told));
        }
        for (_, sub) in state.relays.drain() {
            let _ = sub.out.send(Out::End(reason.clone()));
        }
        state.calls.clear();
        state.waiting.clear();
        drop(state);
        self.departed();
    }

    fn members_changed(&self) {
        let now = self.machine.upstreams();
        let changed = match &self.state().up {
            Some(up) => fastctx_net::tree::moved(&now, &up.endpoint, &up.tried),
            None => true,
        };
        if changed {
            self.rerouted.notify_waiters();
        }
    }
}

/// What a machine leaving below undoes: its upstream session, the calls to
/// and from it, and the data connections waiting for it. Whoever waits for
/// machines to leave is woken by the caller (`Shared::departed`).
fn gone(state: &mut State, name: &MachineName, tag: u64) {
    if let Some(up) = &state.up {
        let _ = up.out.send(Up::Detach { name: name.clone(), tag });
    }
    calls::left(state, name, tag);
    state.waiting.retain(|_, waiting| waiting.other != *name);
}
