//! Moving the hub to another machine (`world-network.md`, 把中枢搬到别处).
//!
//! A hub is the few things in its directory: its id, its certificate and
//! key, and its login secrets are its identity; the rest are its records.
//! Moving the whole directory keeps the identity, so every member keeps the
//! certificate it pinned and its login, and only learns a new address.
//!
//! Only the super administrator moves the hub, and only to one of its own
//! machines, online (`requests`). That machine sees the move in its view,
//! takes the identity from the hub, makes the new directory and has the
//! hub's system service installed there, which takes administrator rights
//! on that machine; the new hub then stands by (`standby`), refusing
//! everything but the old hub. The old hub dials it at its new address,
//! copies the records across while it still serves, then stops taking
//! anything, copies what changed meanwhile and hands over (`out`): the new
//! place checks that it holds exactly what the old hub held and takes over.
//!
//! From then on the old place only forwards connections to the new one,
//! byte for byte (`old`): a machine away during the move comes back to the
//! old address, reaches the new hub through it and learns the new address
//! there, as every connected machine does at once. The new hub counts the
//! machines still coming through the old place (`follow`) and tells it to
//! stop when none is left, or when the super administrator says so; the
//! old data stays, disabled, until the super administrator deletes it.
//!
//! Until the old hub has recorded the switch, it is the hub: a move cut
//! off on either side, by a crash or a stop, either finishes when the old
//! hub tries again, or is called off with nothing lost. The new place
//! serves no member until the old hub confirmed the switch, so the two
//! never both take changes.
//!
//! ```text
//! <old hub>/moving-out.json   the move away, with the move's own key (`Outgoing`)
//! <new hub>/arriving.json     the move in, until the old hub confirmed it (`Arriving`)
//! <new hub>/moves.json        every move that brought the hub here (`Incoming`), a record like the others
//! ```

pub(crate) mod files;
pub(crate) mod follow;
pub(crate) mod frames;
pub(crate) mod old;
pub(crate) mod out;
pub(crate) mod requests;
mod snapshot;
pub(crate) mod standby;

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use fastctx_net::sign::{PublicKey, Secret, SecretKey};
use fastctx_types::hub_move::{Following, HubMove, MoveStage, OldPlace, Waiting};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, mpsc, watch};

use crate::store::read;

pub(crate) const OUTGOING: &str = "moving-out.json";
pub(crate) const ARRIVING: &str = "arriving.json";
pub(crate) const MOVES: &str = "moves.json";
/// A record like the others: a new hub tells what the old one could not.
pub(crate) const CALLED_OFF: &str = "called-off.json";
/// How a new place standing by refuses a move that does not arrive there,
/// as one called off and replaced by another.
pub(crate) const NOT_ARRIVING: &str = "this move does not arrive here";
/// Left where a hub's data was deleted: a hub that moved away, or a move
/// that was to bring it here and was called off.
pub(crate) const RETIRED: &str = "retired.json";

/// A move of the hub away from here, as the old hub keeps it. Its key stays
/// here: the new place holds only its public half.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Outgoing {
    pub id: String,
    pub to: MachineName,
    pub addresses: Vec<SocketAddr>,
    pub from: Vec<SocketAddr>,
    pub by: AccountName,
    pub started: Timestamp,
    pub key: Secret,
    pub stage: MoveStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<Waiting>,
    /// The new place made the new hub's directory: a hub may stand by there.
    #[serde(default)]
    pub prepared: bool,
    /// The new place said the hub's service is installed there.
    #[serde(default)]
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switched: Option<Timestamp>,
    /// After the switch: how this place stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<OldPlace>,
    /// What the new hub said last of who has not followed: machines, devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<(usize, usize)>,
}

impl Outgoing {
    /// Whether the move is under way: not yet switched, nor called off.
    pub fn pending(&self) -> bool {
        self.switched.is_none() && self.stage != MoveStage::Cancelled
    }

    pub fn key(&self) -> SecretKey {
        SecretKey::from_seed(&self.key.0)
    }

    /// The move as the super administrator sees it before the switch.
    pub fn view(&self) -> HubMove {
        HubMove {
            id: self.id.clone(),
            to: self.to.clone(),
            addresses: self.addresses.clone(),
            from: self.from.clone(),
            by: self.by.clone(),
            started: self.started,
            stage: self.stage,
            error: self.error.clone(),
            waiting: self.waiting.clone(),
            switched: self.switched,
            following: None,
        }
    }
}

/// A move in, at the new place, until the old hub confirms the switch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Arriving {
    pub id: String,
    /// The public half of the move's key.
    pub mover: PublicKey,
    pub to: MachineName,
    pub addresses: Vec<SocketAddr>,
    pub from: Vec<SocketAddr>,
    /// The records are taken over, and the new place waits for the old hub
    /// to confirm it recorded the switch.
    #[serde(default)]
    pub committed: bool,
}

/// A move that brought the hub here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Incoming {
    pub id: String,
    pub to: MachineName,
    pub addresses: Vec<SocketAddr>,
    pub from: Vec<SocketAddr>,
    pub by: AccountName,
    pub started: Timestamp,
    pub switched: Timestamp,
    pub mover: PublicKey,
    /// Every member when the hub switched, and whether it is a machine.
    pub members: BTreeMap<MachineName, bool>,
    /// The members that have connected here other than through the old place since.
    #[serde(default)]
    pub followed: BTreeSet<MachineName>,
    pub old: OldPlace,
}

impl Incoming {
    /// The members that existed at the switch, are members still, and have
    /// not followed: machines, then devices.
    pub fn remaining(&self, directory: &fastctx_net::directory::Directory) -> (Vec<MachineName>, Vec<MachineName>) {
        let mut machines = Vec::new();
        let mut devices = Vec::new();
        for (name, machine) in &self.members {
            if self.followed.contains(name) || directory.member(name).is_none() {
                continue;
            }
            if *machine { machines.push(name.clone()) } else { devices.push(name.clone()) }
        }
        (machines, devices)
    }

    pub fn forwarding(&self) -> bool {
        matches!(self.old, OldPlace::Forwarding)
    }

    pub fn view(&self, directory: &fastctx_net::directory::Directory, old_connected: bool) -> HubMove {
        let (machines, devices) = self.remaining(directory);
        HubMove {
            id: self.id.clone(),
            to: self.to.clone(),
            addresses: self.addresses.clone(),
            from: self.from.clone(),
            by: self.by.clone(),
            started: self.started,
            stage: if self.forwarding() { MoveStage::Following } else { MoveStage::Done },
            error: None,
            waiting: None,
            switched: Some(self.switched),
            following: Some(Following { machines, devices, old: self.old.clone(), old_connected }),
        }
    }
}

/// What a hub directory runs as.
pub(crate) enum Mode {
    /// The hub, serving its members; a move away may be under way.
    Serving,
    /// The new place of a move, until the old hub confirms the switch.
    Standby(Arriving),
    /// The old place after a move: forwarding, or done with that.
    Old(Outgoing),
    /// No hub any more: its data was deleted here.
    Retired,
}

pub(crate) fn mode(dir: &Path) -> io::Result<Mode> {
    match read::<Outgoing>(&dir.join(OUTGOING)) {
        Ok(outgoing) if outgoing.switched.is_some() => return Ok(Mode::Old(outgoing)),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if retired(dir)? {
        return Ok(Mode::Retired);
    }
    match read::<Arriving>(&dir.join(ARRIVING)) {
        Ok(arriving) => Ok(Mode::Standby(arriving)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Mode::Serving),
        Err(error) => Err(error),
    }
}

/// Whether `dir` holds nothing of a hub but the note that its data was
/// deleted here.
pub(crate) fn retired(dir: &Path) -> io::Result<bool> {
    dir.join(RETIRED).try_exists()
}

#[derive(Serialize, Deserialize)]
struct Retired {
    at: Timestamp,
    why: String,
}

/// Deletes the hub's data in `dir`, all but its log, leaves a note why,
/// and has its system service removed. The process then waits idle, as a
/// service whose removal failed keeps starting it.
pub(crate) fn retire(dir: &Path, why: &str, log: &crate::log::Log, leave: &crate::Leave) {
    match clear(dir, why) {
        Ok(()) => log.line(format_args!("the hub's data here is deleted: {why}")),
        Err(error) => log.line(format_args!("deleting the hub's data here: {error}")),
    }
    match (leave.remove_service)(dir) {
        Ok(said) => log.line(format_args!("{said}")),
        Err(error) => log.line(format_args!("removing the hub's service: {error}")),
    }
}

pub(crate) fn clear(dir: &Path, why: &str) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("hub.log") || name == RETIRED || name == OUTGOING {
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    let note = Retired { at: Timestamp::now(), why: why.to_string() };
    fastctx_platform::publish(&dir.join(RETIRED), &crate::store::json(&note)?, fastctx_platform::Permissions::Private)?;
    match std::fs::remove_file(dir.join(OUTGOING)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Where `me` reaches the hub now, for its view: empty for a hub never
/// moved, whose members keep what they logged in with. Otherwise the hub's
/// own addresses, after the loopback one for the machine it runs on, and
/// the old places still forwarding, for members that cannot reach it yet.
pub(crate) fn addresses_for(store: &crate::store::Store, me: &MachineName) -> Vec<SocketAddr> {
    let Some(latest) = store.moves.last() else { return Vec::new() };
    let mut addresses = Vec::new();
    let listen = store.hub.listen;
    if latest.to == *me && (listen.ip().is_unspecified() || listen.ip().is_loopback()) {
        let loopback = match listen.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        };
        addresses.push(SocketAddr::new(loopback, listen.port()));
    }
    let forwarding =
        store.moves.iter().rev().filter(|incoming| incoming.forwarding()).flat_map(|incoming| &incoming.from);
    for address in store.hub.public.iter().chain(forwarding) {
        if !addresses.contains(address) {
            addresses.push(*address);
        }
    }
    addresses
}

/// A move's id, at random.
pub(crate) fn new_id() -> io::Result<String> {
    Ok(fastctx_net::sign::random::<6>()?.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// What a serving hub knows of moves while it runs: whether it takes
/// anything now, and the connections that would change its records.
pub(crate) struct Moving {
    /// Set while the old hub hands over: nothing is taken any more.
    frozen: AtomicBool,
    /// Connections that may change the records, under way.
    active: AtomicUsize,
    idle: Notify,
    /// Woken when the hub takes changes again after a switch that did not
    /// finish.
    thawed: Notify,
    /// Set when the hub stops serving here: every connection still open ends.
    stop: watch::Sender<bool>,
    /// Wakes the move under way to try its step again now.
    pub poke: Notify,
    /// Whether a task drives the move away.
    pub driving: AtomicBool,
    /// Told when the old hub recorded the switch: it leaves serving.
    pub switched: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// The old places connected now, by the id of the move that left them.
    pub olds: Mutex<std::collections::HashMap<String, mpsc::UnboundedSender<frames::ToOld>>>,
}

/// How long a hub that stops taking anything waits for what it took to be
/// done, a letter being written among it: past it, a move does not go on.
const SETTLING: std::time::Duration = std::time::Duration::from_secs(60);

/// A connection that may change the records, or work it began that may,
/// counted while it runs.
pub(crate) struct Active(std::sync::Arc<crate::Hub>);

impl Drop for Active {
    fn drop(&mut self) {
        if self.0.moving.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.moving.idle.notify_waiters();
        }
    }
}

impl Moving {
    pub fn new(switched: tokio::sync::oneshot::Sender<()>) -> Self {
        Self {
            frozen: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            idle: Notify::new(),
            thawed: Notify::new(),
            stop: watch::channel(false).0,
            poke: Notify::new(),
            driving: AtomicBool::new(false),
            switched: Mutex::new(Some(switched)),
            olds: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn frozen(&self) -> bool {
        self.frozen.load(Ordering::SeqCst)
    }

    /// Refuses new work before existing connections and their children drain.
    pub fn stop_taking(&self) {
        self.frozen.store(true, Ordering::SeqCst);
    }

    /// Counts a connection that may change the records, unless the hub
    /// takes nothing now.
    pub fn enter(hub: &std::sync::Arc<crate::Hub>) -> Option<Active> {
        hub.moving.active.fetch_add(1, Ordering::SeqCst);
        let active = Active(hub.clone());
        if hub.moving.frozen() { None } else { Some(active) }
    }

    /// Counts work that something taken began, which may change the records
    /// after it is gone, as writing a letter its session sent or deleting
    /// those a member took: whoever waits for the records to be still waits
    /// for it too.
    pub fn hold(hub: &std::sync::Arc<crate::Hub>) -> Active {
        hub.moving.active.fetch_add(1, Ordering::SeqCst);
        Active(hub.clone())
    }

    /// Resolves once nothing that may change the records is under way:
    /// whether that came by `deadline`.
    pub async fn settled(&self, deadline: tokio::time::Instant) -> bool {
        loop {
            let idle = self.idle.notified();
            if self.active.load(Ordering::SeqCst) == 0 {
                return true;
            }
            if tokio::time::timeout_at(deadline, idle).await.is_err() {
                return self.active.load(Ordering::SeqCst) == 0;
            }
        }
    }

    /// Stops taking anything, ends every member's session and the
    /// connections of old places of earlier moves, waits until no
    /// connection that may change the records is left, nor anything they
    /// began, and keeps the records of where the letters stand as they are
    /// then: their writer may not have caught up with the last changes.
    /// What is not done within `SETTLING` holds the move back.
    pub async fn freeze(hub: &crate::Hub) -> Result<(), String> {
        hub.moving.stop_taking();
        let sessions: Vec<_> = hub.sessions().values().map(|session| session.out.clone()).collect();
        for out in sessions {
            let _ = out.send(crate::Out::End(fastctx_net::wire::Refusal::ShuttingDown));
        }
        hub.moving.olds.lock().expect("never poisoned").clear();
        if !hub.moving.settled(tokio::time::Instant::now() + SETTLING).await {
            return Err(format!("what the hub took was not done within {} s of it stopping", SETTLING.as_secs()));
        }
        hub.store().save_letters_now().map_err(|error| format!("recording where the letters stand: {error}"))?;
        crate::updates::flush(hub).await
    }

    pub fn thaw(&self) {
        self.frozen.store(false, Ordering::SeqCst);
        self.thawed.notify_waiters();
    }

    /// Resolves once the hub takes changes, at once when it does now.
    pub async fn unfrozen(&self) {
        loop {
            let thawed = self.thawed.notified();
            if !self.frozen() {
                return;
            }
            thawed.await;
        }
    }

    /// Ends every connection still open here.
    pub fn stop(&self) {
        self.stop.send_replace(true);
    }

    /// Resolves once the hub stops serving here.
    pub async fn stopped(&self) {
        let mut stop = self.stop.subscribe();
        let _ = stop.wait_for(|stopped| *stopped).await;
    }

    pub fn old_connected(&self, id: &str) -> bool {
        self.olds.lock().expect("never poisoned").contains_key(id)
    }
}
