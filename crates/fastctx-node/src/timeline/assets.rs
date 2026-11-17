//! A conversation's timeline in the asset library (design §10, §11): made
//! from the history the content database's timeline is made from, and
//! written as the `Timeline` kind into the conversation's scope, its stream
//! ordered by this machine. Each event is one item named by what it says it
//! is — an item, words handed over, a turn's start or end — so an event that
//! changes is a new revision of the same item; an event that says no such
//! thing is named by its content. What is written is what changed since the
//! timeline here, item by item.
//!
//! The scope is the conversation's under the one account it keeps its
//! records with: its opener, and the machine it was born on when it was born
//! shared. Begun here, it is proposed to that account's hub, which describes
//! it at the driver's next round.

use super::{order, preview};
use crate::Node;
use crate::assets::Proposal;
use fastctx_content::v2::{EVENT, Entry, Kind as _, LEASE, Library, Network, OVERSIZED, PAGE, Timeline, Write, Writer};
use fastctx_content::RECORD_BYTES;
use fastctx_types::access::HubId;
use fastctx_types::assets::Readers;
use fastctx_types::ipc::Failure;
use fastctx_types::ResidentId;
use fastctx_types::residents::About;
use fastctx_types::session::SessionEvent;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Items one commit writes at most, and about how many bytes.
const WRITES_AT_ONCE: usize = 1_024;
const BYTES_AT_ONCE: usize = 4 * 1024 * 1024;

/// Objects put at once on this machine, and the bytes of them in flight: a
/// put waits on the disk twice (the file, then its record), so a few at once
/// keep the disk busy; more would lengthen each put that a user waits on.
const PUTS_AT_ONCE: usize = 4;
const PUT_BYTES_IN_FLIGHT: usize = 32 * 1024 * 1024;
/// Writes of one timeline lined up before their objects are put: about this
/// many bytes of objects, or this many writes.
const QUEUED_PUT_BYTES: usize = 8 * 1024 * 1024;
const QUEUED_WRITES: usize = 256;

/// The objects being put on this machine: how many, how many bytes.
static PUTTING: (std::sync::Mutex<(usize, usize)>, std::sync::Condvar) = (std::sync::Mutex::new((0, 0)), std::sync::Condvar::new());

/// A place among the objects being put (`PUTS_AT_ONCE`, `PUT_BYTES_IN_FLIGHT`),
/// held while one of `bytes` is put; one larger than the budget goes alone.
struct Putting(usize);

impl Putting {
    fn take(bytes: usize) -> Self {
        let (busy, freed) = &PUTTING;
        let mut busy = busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while busy.0 >= PUTS_AT_ONCE || (busy.0 > 0 && busy.1 + bytes > PUT_BYTES_IN_FLIGHT) {
            busy = freed.wait(busy).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        busy.0 += 1;
        busy.1 += bytes;
        Self(bytes)
    }
}

impl Drop for Putting {
    fn drop(&mut self) {
        let (busy, freed) = &PUTTING;
        let mut busy = busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        busy.0 -= 1;
        busy.1 -= self.0;
        freed.notify_all();
    }
}

/// A write of the timeline lined up, with the object to put first where it
/// has one: the encoded event and what it is taken for.
struct Queued {
    write: Write<Timeline>,
    put: Option<(Vec<u8>, fastctx_content::ObjectRef)>,
    /// Its object's lease, where it was put already (`Early`).
    leased: Option<fastctx_content::v2::Lease>,
    bytes: usize,
}

/// Bytes of objects waiting to be put early (`Early`) at most.
const EARLY_BYTES: usize = 12 * 1024 * 1024;

/// The objects of a whole reading put as their events are read, before the
/// timeline is written (design §28.51), so that putting them — the most of
/// the time a large reading takes — goes on beside the reading: those that
/// wait within a budget, put by a few threads under the node's budget of
/// puts (`Putting`); one here already is not put. The rest are put as the
/// timeline is written. The lease of each object put is kept until a write
/// takes it; those no write took are let go when it is dropped.
pub(crate) struct Early {
    shared: Arc<EarlyShared>,
    workers: Vec<std::thread::JoinHandle<()>>,
    /// Counted among the readings kept (`spill::kept`) until its puts are
    /// done and its leases let go: a new holder is admitted only after.
    entered: Option<super::spill::Entered>,
}

struct EarlyShared {
    library: Library,
    holder: String,
    state: std::sync::Mutex<EarlyState>,
    changed: std::sync::Condvar,
}

#[derive(Default)]
struct EarlyState {
    /// Each event offered, as JSON.
    waiting: std::collections::VecDeque<Vec<u8>>,
    /// The objects put already or being put, by hash: none is put twice.
    seen: HashSet<[u8; 32]>,
    bytes: usize,
    /// Taken from `waiting`, not done yet.
    working: usize,
    closed: bool,
    leases: HashMap<fastctx_content::ObjectRef, fastctx_content::v2::Lease>,
}

impl Early {
    /// Puts begun as offered, into `library` for `resident`'s timeline; none
    /// while a new holder of the machine is being admitted.
    pub(crate) fn new(library: Library, resident: &ResidentId) -> Option<Self> {
        let entered = super::spill::enter()?;
        let shared = Arc::new(EarlyShared {
            library,
            holder: format!("timeline:{resident}"),
            state: std::sync::Mutex::new(EarlyState::default()),
            changed: std::sync::Condvar::new(),
        });
        let workers: Vec<_> = (0..PUTS_AT_ONCE)
            .filter_map(|_| {
                let shared = shared.clone();
                std::thread::Builder::new().name("timeline-objects".into()).spawn(move || shared.work()).ok()
            })
            .collect();
        // No thread to put them: nothing is taken to be put, all is left to
        // the write.
        if workers.is_empty() {
            shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).closed = true;
        }
        Some(Self { shared, workers, entered: Some(entered) })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, EarlyState> {
        self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Offers `encoded`, an event too large for a record, to be put now: left
    /// to the write where as many bytes as the budget wait already.
    pub(crate) fn offer(&self, encoded: &[u8]) {
        let mut state = self.state();
        if state.closed || (state.bytes > 0 && state.bytes + encoded.len() > EARLY_BYTES) {
            return;
        }
        state.bytes += encoded.len();
        state.waiting.push_back(encoded.to_vec());
        self.shared.changed.notify_all();
    }

    /// No more offers: what waits is put still.
    pub(crate) fn close(&self) {
        self.state().closed = true;
        self.shared.changed.notify_all();
    }

    /// The lease of `object` where it was put here, once all that was
    /// offered is done.
    fn take(&self, object: &fastctx_content::ObjectRef) -> Option<fastctx_content::v2::Lease> {
        let mut state = self.state();
        state.closed = true;
        self.shared.changed.notify_all();
        while !state.waiting.is_empty() || state.working > 0 {
            state = self.shared.changed.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.leases.remove(object)
    }
}

impl EarlyShared {
    fn work(&self) {
        loop {
            let encoded = {
                let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                loop {
                    if let Some(encoded) = state.waiting.pop_front() {
                        state.bytes -= encoded.len();
                        state.working += 1;
                        break encoded;
                    }
                    if state.closed {
                        return;
                    }
                    state = self.changed.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            };
            // Offered twice (an event told again), an object is put once.
            let hash: [u8; 32] = Sha256::digest(&encoded).into();
            let again = !self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).seen.insert(hash);
            let lease = if again { None } else { self.put(&encoded, &hash) };
            drop(encoded);
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.working -= 1;
            if let Some(lease) = lease {
                match state.leases.entry(lease.object) {
                    std::collections::hash_map::Entry::Vacant(vacant) => {
                        vacant.insert(lease);
                    }
                    std::collections::hash_map::Entry::Occupied(_) => {
                        let _ = self.library.release(lease);
                    }
                }
            }
            self.changed.notify_all();
        }
    }

    /// Puts `encoded`, the object `hash`, unless it is here already: its
    /// lease. One that fails is left to the write, which puts it again.
    fn put(&self, encoded: &[u8], hash: &[u8; 32]) -> Option<fastctx_content::v2::Lease> {
        if self.library.size(hash).ok().flatten().is_some() {
            return None;
        }
        let _putting = Putting::take(encoded.len());
        self.library.put(&mut &encoded[..], &self.holder, LEASE).ok()
    }
}

impl Drop for Early {
    fn drop(&mut self) {
        {
            let mut state = self.state();
            state.closed = true;
            state.waiting.clear();
            state.bytes = 0;
        }
        self.shared.changed.notify_all();
        let workers = std::mem::take(&mut self.workers);
        fastctx_platform::roles::in_place("waiting for a timeline's objects to be put", || {
            for worker in workers {
                let _ = worker.join();
            }
        });
        let leases = std::mem::take(&mut self.state().leases);
        for lease in leases.into_values() {
            let _ = self.shared.library.release(lease);
        }
        drop(self.entered.take());
    }
}

/// Each write's lease, where it has an object.
type Leases = Vec<Option<fastctx_content::v2::Lease>>;

/// Puts the objects of the writes lined up, a few at once (`Putting`): each
/// write's lease, in their order. One failed, the leases of the others are
/// let go.
fn put_all(library: &Library, resident: &ResidentId, queued: &mut [Queued]) -> Result<Leases, Failure> {
    let puts: Vec<usize> = queued.iter().enumerate().filter(|(_, queued)| queued.put.is_some()).map(|(at, _)| at).collect();
    let mut leases: Leases = queued.iter_mut().map(|queued| queued.leased.take()).collect();
    let queued: &[Queued] = queued;
    if !puts.is_empty() {
        let holder = format!("timeline:{resident}");
        let next = std::sync::atomic::AtomicUsize::new(0);
        let put: std::sync::Mutex<Vec<(usize, Result<fastctx_content::v2::Lease, Failure>)>> = std::sync::Mutex::new(Vec::with_capacity(puts.len()));
        let lined: &[Queued] = queued;
        std::thread::scope(|scope| {
            for _ in 0..PUTS_AT_ONCE.min(puts.len()) {
                scope.spawn(|| {
                    while let Some(&at) = puts.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                        let (encoded, object) = lined[at].put.as_ref().expect("lined up with an object");
                        let result = {
                            let _putting = Putting::take(encoded.len());
                            library.put(&mut encoded.as_slice(), &holder, LEASE).map_err(failure)
                        };
                        let result = result.and_then(|lease| {
                            if lease.object == *object { Ok(lease) } else { Err(failure("an event's object is not what it was taken for")) }
                        });
                        let failed = result.is_err();
                        put.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((at, result));
                        if failed {
                            // None after a failure is begun.
                            next.store(usize::MAX / 2, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        let mut put = put.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
        put.sort_by_key(|(at, _)| *at);
        let mut failed = None;
        for (at, result) in put {
            match result {
                Ok(lease) => leases[at] = Some(lease),
                Err(error) => {
                    failed.get_or_insert(error);
                }
            }
        }
        // One failed: the leases of the others, put but referred to by no
        // record, are let go now rather than holding their objects until they
        // run out.
        if let Some(error) = failed {
            for lease in leases.into_iter().flatten() {
                let _ = library.release(lease);
            }
            return Err(error);
        }
    }
    Ok(leases)
}

/// What is put of a window of writes: the window and its leases.
type Put = (Vec<Queued>, Result<Leases, Failure>);

/// Writes of the timeline lined up and handed on to `pending` in their
/// order, the objects of each window of them put on another thread
/// (`put_all`): the next window is lined up while one is put, and one is
/// handed on — its records committed — while the next is put. At most one
/// window is being put, one handed on and one lined up.
struct Lining<'s, 'a> {
    library: &'s Library,
    queued: Vec<Queued>,
    bytes: usize,
    send: std::sync::mpsc::SyncSender<Vec<Queued>>,
    done: std::sync::mpsc::Receiver<Put>,
    /// Windows sent to be put, not handed on yet.
    in_flight: usize,
    pending: &'s mut Pending<'a>,
}

impl Lining<'_, '_> {
    /// Lines up `queued`; a window full, it is sent to be put.
    fn push(&mut self, queued: Queued) -> Result<(), Failure> {
        self.bytes += queued.put.as_ref().map_or(0, |(encoded, _)| encoded.len());
        self.queued.push(queued);
        if self.bytes >= QUEUED_PUT_BYTES || self.queued.len() >= QUEUED_WRITES {
            self.send_window()?;
        }
        Ok(())
    }

    /// Sends the window lined up to be put once the one being put is done,
    /// then hands that one on.
    fn send_window(&mut self) -> Result<(), Failure> {
        if self.queued.is_empty() {
            return Ok(());
        }
        let window = std::mem::take(&mut self.queued);
        self.bytes = 0;
        // Taken once the window before is put; that one is handed on now.
        self.send.send(window).map_err(|_| failure("the timeline's objects stopped being put"))?;
        self.in_flight += 1;
        while self.in_flight > 1 {
            self.hand_on()?;
        }
        Ok(())
    }

    /// Hands on the next window put, its writes in their order with their
    /// leases. One not handed on lets the leases of those after it go.
    fn hand_on(&mut self) -> Result<(), Failure> {
        let library = self.library;
        let (window, leases) = self.done.recv().map_err(|_| failure("the timeline's objects stopped being put"))?;
        self.in_flight -= 1;
        let leases = leases?;
        let mut handing = window.into_iter().zip(leases);
        while let Some((queued, lease)) = handing.next() {
            if let Err(error) = self.pending.push(queued.write, lease, queued.bytes) {
                for (_, lease) in handing {
                    if let Some(lease) = lease {
                        let _ = library.release(lease);
                    }
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Every write lined up handed on.
    fn finish(&mut self) -> Result<(), Failure> {
        self.send_window()?;
        while self.in_flight > 0 {
            self.hand_on()?;
        }
        Ok(())
    }

    /// After a failure: the windows still being put are waited for, and
    /// their leases and those of the writes lined up, which no record refers
    /// to, let go.
    fn abandon(&mut self) {
        let library = self.library;
        for queued in self.queued.drain(..) {
            if let Some(lease) = queued.leased {
                let _ = library.release(lease);
            }
        }
        while self.in_flight > 0 {
            self.in_flight -= 1;
            if let Ok((_, Ok(leases))) = self.done.recv() {
                for lease in leases.into_iter().flatten() {
                    let _ = library.release(lease);
                }
            }
        }
    }
}

/// Runs `make`, which lines up the timeline's writes (`Lining`), with their
/// objects put on another thread as it goes: what it returns, once every
/// write it lined up is handed on to `pending`.
fn lined<R>(
    library: &Library,
    resident: &ResidentId,
    pending: &mut Pending<'_>,
    make: impl FnOnce(&mut Lining<'_, '_>) -> Result<R, Failure>,
) -> Result<R, Failure> {
    std::thread::scope(|scope| {
        let (send, windows) = std::sync::mpsc::sync_channel::<Vec<Queued>>(0);
        let (put, done) = std::sync::mpsc::sync_channel::<Put>(1);
        scope.spawn(move || {
            for mut window in windows {
                let leases = put_all(library, resident, &mut window);
                if put.send((window, leases)).is_err() {
                    break;
                }
            }
        });
        let mut lining = Lining { library, queued: Vec::new(), bytes: 0, send, done, in_flight: 0, pending };
        let made = make(&mut lining).and_then(|made| lining.finish().map(|()| made));
        if made.is_err() {
            lining.abandon();
        }
        // Its sender gone, the thread that puts ends.
        drop(lining);
        made
    })
}

/// The conversations whose timeline is being written here now, and the
/// signal that one is done.
static WRITING: (std::sync::Mutex<std::collections::BTreeSet<String>>, std::sync::Condvar) =
    (std::sync::Mutex::new(std::collections::BTreeSet::new()), std::sync::Condvar::new());

/// One conversation's timeline written by one writer at a time: a build its
/// job gave up on, still writing, is done before the next one reads what is
/// here (AA08).
struct Writing(String);

impl Writing {
    fn take(resident: &ResidentId) -> Self {
        let (busy, done) = &WRITING;
        let mut busy = busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while busy.contains(resident.as_str()) {
            busy = done.wait(busy).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        busy.insert(resident.to_string());
        Self(resident.to_string())
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        let (busy, done) = &WRITING;
        busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.0);
        done.notify_all();
    }
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Where a conversation's timeline goes: its scope, and the hub it is
/// described by, under the term this machine is on that hub's network.
pub(super) struct Placed {
    pub(super) scope: String,
    pub(super) hub: HubId,
    pub(super) proposal: Proposal,
    pub(super) network: Network,
}

/// The hub and scope `resident`'s timeline goes to, if it keeps its records
/// with an account on a network this machine's assets are on.
pub(crate) fn scope_of(node: &Node, resident: &ResidentId) -> Result<Option<(HubId, String)>, Failure> {
    let Some((about, _)) = node.residents.about(resident) else { return Ok(None) };
    Ok(placed(node, &about)?.map(|placed| (placed.hub, placed.scope)))
}

/// The name a conversation's producer keeps where it stands under.
pub(super) fn checkpoint(resident: &ResidentId) -> String {
    checkpoint_of(resident.as_str())
}

/// The same, of the conversation named `resident` in a scope's key.
pub(crate) fn checkpoint_of(resident: &str) -> String {
    format!("timeline/{resident}")
}

/// Where the conversation's timeline goes, if it keeps its records with one
/// account on a network this machine's assets are on: a conversation
/// several accounts share alike has none yet.
pub(super) fn placed(node: &Node, about: &About) -> Result<Option<Placed>, Failure> {
    let Some(account) = crate::placement::account(node, &about.acting, None)? else { return Ok(None) };
    let Some(network) = crate::assets::membership::network(node, &account.hub) else { return Ok(None) };
    // The machine it was born on stays its place however it moves: a moved
    // conversation's timeline goes on there once its ordering is handed
    // over (design §8, plan 3-4), never in a scope of its own here (BB08).
    let born_on = crate::record_ownership::origin(node, about);
    let born_shared = about.born_shared.get(account.hub.as_str()).cloned();
    let (readers, scope) = scope_for(node, &account.hub, &account.account, born_on, &about.id, born_shared)?;
    let classes = fastctx_types::assets::classes_for(&readers, &scope);
    Ok(Some(Placed { scope, hub: account.hub.clone(), proposal: Proposal { hub: account.hub.to_string(), readers, classes }, network }))
}

/// The readers and key of the scope of the conversation `id` of `account`
/// born on `born_on`: one opened on a machine shared whole is the body's —
/// read by its members, as they are or were frozen, and by the account that
/// opened it (§6.2) — any other the conversation's. One that has a scope
/// here already keeps it, however the machine is shared now. Whether it was
/// shared whole is what the machine it was born on knew as it was born
/// (`born_shared`, §29, 2.1), judged only for a birth of that machine's own:
/// one born assisting an account has no `born_on` and stays its own.
fn scope_for(
    node: &Node,
    hub: &fastctx_types::access::HubId,
    account: &fastctx_types::AccountName,
    born_on: Option<fastctx_types::MachineName>,
    id: &ResidentId,
    born_shared: Option<Option<String>>,
) -> Result<(Readers, String), Failure> {
    let at_birth = born_on.as_ref().and(born_shared);
    let conversation = Readers::Conversation { openers: vec![account.clone()], born_on: born_on.clone() };
    let conversation_key = format!("{}{}", conversation.key_prefix(hub.as_str()), id);
    // One that has a scope here already keeps it, though the machine it was
    // born on left the network since and its body is known by none now
    // (AT11).
    if let Some(assets) = node.assets.as_ref() {
        let tail = format!("/{id}");
        let keys = assets.library.keys_ending(&tail).map_err(|error| Failure::new(error.to_string()))?;
        if keys.contains(&conversation_key) {
            return Ok((conversation, conversation_key));
        }
        // A body's this account opened, as the hub described it, and only
        // one: another's of the same id is not this conversation (AU05).
        let opened_here = |key: &String| {
            assets.library.held_listing(key).ok().flatten().is_some_and(|listing| {
                serde_json::from_slice::<fastctx_types::assets::Descriptor>(&listing.description).is_ok_and(|described| {
                    matches!(&described.readers, Readers::Body { opener, .. } if opener == account)
                })
            })
        };
        // Of the body the machine it was born on was as it was born, never
        // another machine's of the same id the account opened there (AV08,
        // AW02); one born here before that was kept, of the body this
        // machine is now.
        let mine: Option<std::collections::BTreeSet<String>> = match &at_birth {
            Some(body) => Some(body.iter().cloned().collect()),
            None if born_on.as_ref().is_some_and(|machine| node.is_me(machine)) => {
                Some(super::born_shared::body_here(node, hub).into_iter().collect())
            }
            None => None,
        };
        let bodies: Vec<&str> = keys
            .iter()
            .filter(|key| opened_here(key))
            .filter_map(|key| key.strip_prefix("b/").and_then(|rest| rest.strip_suffix(tail.as_str())))
            .filter(|body| !body.contains('/'))
            .filter(|body| mine.as_ref().is_none_or(|mine| mine.contains(*body)))
            .collect();
        if let [body] = bodies.as_slice() {
            let readers = Readers::Body { body: body.to_string(), opener: account.clone() };
            let key = format!("{}{}", readers.key_prefix(hub.as_str()), id);
            return Ok((readers, key));
        }
    }
    let (Some(machine), Some(link), Some(assets)) = (born_on, node.link_of(hub), node.assets.as_ref()) else {
        return Ok((conversation, conversation_key));
    };
    let (body, shared) = {
        let view = link.view();
        let directory = &view.directory;
        let machine = directory.canonical(&machine);
        (directory.body_of(machine), directory.shares.iter().any(|share| &share.machine == machine && share.active()))
    };
    // The body its machine was as it was born, however late its scope is
    // made now (N02); for one born before that was kept, as it is now.
    let (body, shared) = match at_birth {
        Some(Some(then)) => (Some(then), true),
        Some(None) => return Ok((conversation, conversation_key)),
        None => (body, shared),
    };
    let Some(body) = body else { return Ok((conversation, conversation_key)) };
    let readers = Readers::Body { body, opener: account.clone() };
    let body_key = format!("{}{}", readers.key_prefix(hub.as_str()), id);
    let knows = |key: &str| assets.library.knows(key).map_err(|error| Failure::new(error.to_string()));
    if knows(&body_key)? || (shared && !knows(&conversation_key)?) {
        return Ok((readers, body_key));
    }
    Ok((conversation, conversation_key))
}

/// What the timeline here holds now, item by item.
fn held(library: &Library, scope: &str) -> Result<HashMap<Vec<u8>, Entry>, Failure> {
    let mut held = HashMap::new();
    let mut after = None;
    loop {
        let page = library.page(scope, Timeline::STREAM, after.clone(), PAGE).map_err(failure)?;
        let Some(last) = page.last() else { break };
        after = Some((last.order.clone(), last.item.clone()));
        let full = page.len() == PAGE;
        held.extend(page.into_iter().map(|entry| (entry.item.clone(), entry)));
        if !full {
            break;
        }
    }
    Ok(held)
}

/// What a write of a timeline left: how many items it wrote and how many
/// went, and the items of its window with their times and order keys.
pub(super) struct Wrote {
    pub(super) written: usize,
    pub(super) gone: usize,
    pub(super) items: Vec<(Vec<u8>, jiff::Timestamp, u64)>,
    /// The holding period of the scope it was written in.
    pub(super) holding: i64,
}

/// Where a timeline's window begins: the whole timeline, or the events from
/// `since` on, the items written of them before being `before` (identity
/// and time), every item before them ordered below `floor`.
pub(super) enum Span<'a> {
    Whole,
    From { since: jiff::Timestamp, before: &'a [(Vec<u8>, jiff::Timestamp)], floor: u64 },
    /// The whole timeline as far as it could be read, a source of it
    /// unreadable now: what was read is written, nothing is taken away.
    Kept,
    /// As `Kept`, a record of the timeline stopped before a line larger than
    /// World reads (§28.49): an event of that record (in the set, as
    /// `names::frozen` knows it) is written only where the timeline has no
    /// item of it in its index — not over one it has, which a reading short
    /// of the record shows less than it is, nor over a tombstone, which it
    /// would bring back. One known only by its moment (an agent's start,
    /// made from a call and a result the reading stopped before) is written
    /// only where the timeline has none of those records' named items: one
    /// it had, made from all of the record, stands. Each not written keeps
    /// the order key it has, and one placed among them keeps within them. A
    /// set for each record stopped short.
    Added(&'a [HashSet<String>]),
}

/// Brings the conversation's timeline in the asset library to `history` over
/// `span`: the scope begun and proposed if it is not here yet; the window's
/// new and changed items written, those of the window gone from it removed.
/// An event of the history from before the window (the session's title,
/// the note of what could not be read) keeps the place it has. Call on a
/// thread that may block. `expected`, the holding period the window was read
/// on from, if it was: the scope held anew since, nothing is written (GG02).
#[allow(clippy::too_many_arguments)]
pub(super) fn write(
    library: &Library,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    resident: &ResidentId,
    placed: &Placed,
    history: &[SessionEvent],
    span: Span<'_>,
    again: &HashSet<Vec<u8>>,
    expected: Option<i64>,
) -> Result<Wrote, Failure> {
    let _writing = Writing::take(resident);
    let mut since_phase = std::time::Instant::now();
    let holding = holding(library, placed, expected)?;
    let identities = super::names::identities(history);
    let (since, floor) = match &span {
        Span::Whole | Span::Kept | Span::Added(_) => (None, None),
        Span::From { since, floor, .. } => (Some(*since), Some(*floor)),
    };
    let had: HashMap<Vec<u8>, Entry> = match &span {
        Span::Whole | Span::Kept | Span::Added(_) => held(library, &placed.scope)?,
        // The window's items and those before it, read together.
        Span::From { before, .. } => {
            let mut asked: Vec<Vec<u8>> = identities.iter().chain(before.iter().map(|(identity, _)| identity)).cloned().collect();
            asked.sort_unstable();
            asked.dedup();
            library.items(&placed.scope, Timeline::STREAM, &asked).map_err(failure)?
        }
    };
    let key_of = |identity: &Vec<u8>| had.get(identity).and_then(|entry| entry.order.as_slice().try_into().ok().map(u64::from_be_bytes));
    // The window's events take keys past every item before it; one of its
    // own below that is placed again.
    // An event from before the window keeps the place it has; one new to
    // the timeline, though its time is earlier, takes its place in the
    // window, never the very first (CC14); and so does one told again at
    // every reading where the whole history has it (`again`, N1).
    let early: Vec<bool> = history
        .iter()
        .zip(&identities)
        .map(|(event, identity)| since.is_some_and(|since| event.at < since) && had.contains_key(identity) && !again.contains(identity))
        .collect();
    let window: Vec<usize> = (0..history.len()).filter(|index| !early[*index]).collect();
    // Added: the events of a record stopped short whose items the timeline
    // has, in any state, are not written (`Span::Added`).
    let untouched: HashSet<Vec<u8>> = match &span {
        Span::Added(frozen) => {
            let mut events = history
                .iter()
                .zip(&identities)
                .map(|(event, identity)| (identity.clone(), super::names::frozen(event), super::names::key(event).is_some()));
            untouched(frozen, &mut events, &mut |asked| library.had_items(&placed.scope, Timeline::STREAM, asked).map_err(failure))?
        }
        _ => HashSet::new(),
    };
    phase("had", &mut since_phase);
    let mut keyed: Vec<Option<u64>> = Vec::with_capacity(window.len() + 1);
    if let Some(floor) = floor {
        keyed.push(Some(floor));
    }
    keyed.extend(window.iter().map(|&index| key_of(&identities[index]).filter(|key| floor.is_none_or(|floor| *key > floor))));
    let assigned = order::assign(&keyed);
    let assigned = if floor.is_some() { &assigned[1..] } else { &assigned[..] };
    let mut keys = vec![0u64; history.len()];
    for (&index, key) in window.iter().zip(assigned) {
        keys[index] = *key;
    }
    for index in (0..history.len()).filter(|index| early[*index]) {
        keys[index] = key_of(&identities[index]).unwrap_or(0);
    }
    // Not written, an event left as the timeline has it keeps the key it
    // has: placed between two of them without room, a new one may share a
    // key with the next (as with an unread rest, §28.49).
    let fixed: Vec<Option<u64>> =
        window.iter().map(|&index| if untouched.contains(&identities[index]) { key_of(&identities[index]) } else { None }).collect();
    for (&index, key) in window.iter().zip(&fixed) {
        if let Some(key) = key {
            keys[index] = *key;
        }
    }
    let mut placed_keys: Vec<u64> = window.iter().map(|&index| keys[index]).collect();
    order::within(&mut placed_keys, &fixed);
    for (&index, key) in window.iter().zip(placed_keys) {
        keys[index] = key;
    }
    let present: HashSet<&Vec<u8>> = identities.iter().collect();
    let gone_items: Vec<(&Vec<u8>, &Entry)> = match &span {
        Span::Whole => had.iter().filter(|(identity, _)| !present.contains(identity)).collect(),
        Span::Kept | Span::Added(_) => Vec::new(),
        Span::From { since, before, .. } => before
            .iter()
            .filter(|(identity, at)| at >= since && !present.contains(identity))
            .filter_map(|(identity, _)| had.get_key_value(identity))
            .collect(),
    };
    // Every commit in the period it was read for: held anew meanwhile, the
    // rest is refused, not written into the new one.
    let writer = Writer::<Timeline>::new(library, &placed.scope, Timeline::STREAM).map_err(failure)?.in_holding(holding);
    let mut pending = Pending { library, writer: &writer, signer, writes: Vec::new(), leases: Vec::new(), bytes: 0 };
    let gone = gone_items.len();
    for (identity, entry) in gone_items {
        pending.push(Write { item: identity.clone(), order: entry.order.clone(), value: None, objects: Vec::new() }, None, 0)?;
    }
    // Lined up so that their objects are put a few at once (`lined`); handed
    // on in the history's order.
    let written = lined(library, resident, &mut pending, |lining| {
        let mut written = 0;
        for (index, event) in history.iter().enumerate() {
            if untouched.contains(&identities[index]) {
                continue;
            }
            let encoded = serde_json::to_vec(event).map_err(failure)?;
            let order = keys[index].to_be_bytes().to_vec();
            let (body, object) = body(event, &encoded)?;
            // Compared before anything is put: an unchanged large event
            // writes nothing again (AA11).
            let objects: Vec<fastctx_content::ObjectRef> = object.into_iter().collect();
            if let Some(entry) = had.get(&identities[index])
                && entry.order == order
                && entry.body == body
                && entry.objects == objects
            {
                continue;
            }
            let put = objects.first().map(|object| (encoded, *object));
            let bytes = body.len();
            phase("bodies", &mut since_phase);
            lining.push(Queued { write: Write { item: identities[index].clone(), order, value: Some(body), objects }, put, leased: None, bytes })?;
            phase("waiting on puts and commits", &mut since_phase);
            written += 1;
        }
        Ok(written)
    })?;
    pending.flush()?;
    phase("waiting on puts and commits", &mut since_phase);
    // A tombstone left as it is is not among the window's items.
    let items = window
        .iter()
        .filter(|&&index| !untouched.contains(&identities[index]) || had.contains_key(&identities[index]))
        .map(|&index| (identities[index].clone(), history[index].at, keys[index]))
        .collect();
    Ok(Wrote { written, gone, items, holding })
}

/// How many items of one finished host item a timeline is asked for at a
/// time, each told apart by those before it (`names::again`): asked again
/// past them while some are found (codex-e024d399: a call alike in many
/// records makes as many items).
const AGAIN_ASKED: u32 = 8;
/// How many calls' items are read at once (`refresh_runs`): their bodies,
/// each at most a record's, held only while they are given their facts.
const CALLS_AT_ONCE: usize = 16;

/// The finished items of the calls `calls`, whose runs' facts changed, that
/// stand before the window (ordered at most `floor`), given their facts
/// again where they show others (design §28.53, codex-e024d399 #1): an item
/// settled before the seam is not read again as the window is, and its run
/// goes on after it. Each keeps its place; one past the floor is the
/// window's, read again with it. A few calls at a time, never every one's
/// items at once. Call on a thread that may block, with the conversation's
/// timeline build held; `attach` gives an event its run's facts as they
/// stand (`run_facts::attach`); `expected`, the holding period the window
/// was written in. How many were written.
#[allow(clippy::too_many_arguments)]
pub(super) fn refresh_runs(
    library: &Library,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    resident: &ResidentId,
    placed: &Placed,
    attach: &dyn Fn(&mut SessionEvent) -> Result<(), Failure>,
    calls: &[String],
    floor: u64,
    expected: Option<i64>,
) -> Result<usize, Failure> {
    if calls.is_empty() {
        return Ok(0);
    }
    let _writing = Writing::take(resident);
    let holding = holding(library, placed, expected)?;
    let writer = Writer::<Timeline>::new(library, &placed.scope, Timeline::STREAM).map_err(failure)?.in_holding(holding);
    let mut pending = Pending { library, writer: &writer, signer, writes: Vec::new(), leases: Vec::new(), bytes: 0 };
    let written = lined(library, resident, &mut pending, |lining| {
        let mut written = 0;
        for chunk in calls.chunks(CALLS_AT_ONCE) {
            let mut asking: Vec<([u8; 32], u32)> =
                chunk.iter().map(|call| (super::names::named_base(&format!("item/{call}")), 0)).collect();
            while !asking.is_empty() {
                let mut asked: Vec<Vec<u8>> = asking
                    .iter()
                    .flat_map(|(base, from)| (*from..from + AGAIN_ASKED).map(|before| super::names::again(*base, before).to_vec()))
                    .collect();
                asked.sort_unstable();
                asked.dedup();
                let had = library.items(&placed.scope, Timeline::STREAM, &asked).map_err(failure)?;
                let mut next = Vec::new();
                for (base, from) in &asking {
                    let mut found = false;
                    for before in *from..from + AGAIN_ASKED {
                        let identity = super::names::again(*base, before).to_vec();
                        let Some(entry) = had.get(&identity) else { continue };
                        found = true;
                        if let Some(queued) = refreshed(library, entry, attach, floor)? {
                            lining.push(queued)?;
                            written += 1;
                        }
                    }
                    if found {
                        next.push((*base, from + AGAIN_ASKED));
                    }
                }
                asking = next;
            }
        }
        Ok(written)
    })?;
    pending.flush()?;
    Ok(written)
}

/// Every item before the window that can tell of a run given its facts
/// again where they show others, a page of the timeline at a time: what
/// changed of them is not known call by call — the timeline made by an
/// earlier form, which kept no version its items had, or the facts changed
/// by an older node (`run_facts::changed_since`) — and the history is not
/// read whole for that (codex-e024d399). As `refresh_runs`.
#[allow(clippy::too_many_arguments)]
pub(super) fn refresh_every_run(
    library: &Library,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    resident: &ResidentId,
    placed: &Placed,
    attach: &dyn Fn(&mut SessionEvent) -> Result<(), Failure>,
    floor: u64,
    expected: Option<i64>,
) -> Result<usize, Failure> {
    let _writing = Writing::take(resident);
    let holding = holding(library, placed, expected)?;
    let writer = Writer::<Timeline>::new(library, &placed.scope, Timeline::STREAM).map_err(failure)?.in_holding(holding);
    let mut pending = Pending { library, writer: &writer, signer, writes: Vec::new(), leases: Vec::new(), bytes: 0 };
    let written = lined(library, resident, &mut pending, |lining| {
        let mut written = 0;
        let mut anchor = fastctx_content::Anchor::After(fastctx_content::Place { order: Vec::new(), item: Vec::new() });
        loop {
            let page = library
                .view_page(&placed.scope, Timeline::STREAM, &anchor, super::reads::PAGE_ITEMS, super::reads::PAGE_BYTES)
                .map_err(failure)?;
            let mut past = false;
            for entry in &page.entries {
                if order_key(entry).is_none_or(|key| key > floor) {
                    past = true;
                    break;
                }
                // Only a host's call of World's `run` has its facts given
                // (`run_facts::tells_of_a_run`): told by the event itself,
                // or by what stands for one too large for a record — however
                // its record spells it (codex-e024d399).
                let event = match entry.body.split_first() {
                    Some((&EVENT, encoded)) => serde_json::from_slice::<SessionEvent>(encoded).map_err(failure)?,
                    Some((&OVERSIZED, told)) => {
                        let told: serde_json::Value = serde_json::from_slice(told).map_err(failure)?;
                        match serde_json::from_value::<Option<SessionEvent>>(told["slim"].clone()).ok().flatten() {
                            Some(slim) if !crate::run_facts::tells_of_a_run(&slim) => continue,
                            Some(_) | None => match event_of(library, entry)? {
                                Some(event) => event,
                                None => continue,
                            },
                        }
                    }
                    _ => continue,
                };
                if !crate::run_facts::tells_of_a_run(&event) {
                    continue;
                }
                if let Some(queued) = given(entry, event, attach)? {
                    lining.push(queued)?;
                    written += 1;
                }
            }
            match page.newer {
                Some(place) if !past => anchor = fastctx_content::Anchor::After(place),
                _ => break,
            }
        }
        Ok(written)
    })?;
    pending.flush()?;
    Ok(written)
}

/// An entry's order key.
fn order_key(entry: &Entry) -> Option<u64> {
    entry.order.as_slice().try_into().ok().map(u64::from_be_bytes)
}

/// The write that gives the item `entry` its run's facts as `attach` gives
/// them, where it stands at most at `floor` and they change its record.
fn refreshed(
    library: &Library,
    entry: &Entry,
    attach: &dyn Fn(&mut SessionEvent) -> Result<(), Failure>,
    floor: u64,
) -> Result<Option<Queued>, Failure> {
    if order_key(entry).is_none_or(|key| key > floor) {
        return Ok(None);
    }
    let Some(event) = event_of(library, entry)? else { return Ok(None) };
    given(entry, event, attach)
}

/// The write that gives `event`, the item `entry` holds, its run's facts as
/// `attach` gives them, where they change its record.
fn given(entry: &Entry, mut event: SessionEvent, attach: &dyn Fn(&mut SessionEvent) -> Result<(), Failure>) -> Result<Option<Queued>, Failure> {
    attach(&mut event)?;
    let encoded = serde_json::to_vec(&event).map_err(failure)?;
    let (body, object) = body(&event, &encoded)?;
    let objects: Vec<fastctx_content::ObjectRef> = object.into_iter().collect();
    if entry.body == body && entry.objects == objects {
        return Ok(None);
    }
    let put = objects.first().map(|object| (encoded, *object));
    let bytes = body.len();
    Ok(Some(Queued { write: Write { item: entry.item.clone(), order: entry.order.clone(), value: Some(body), objects }, put, leased: None, bytes }))
}

/// The most an event kept whole in an object is read into memory.
pub(super) const EVENT_OBJECT_BYTES: u64 = 64 << 20;

/// The bytes of an object kept here, read a piece at a time (one read gives
/// at most a mebibyte); none where it is not here.
pub(super) fn object_whole(library: &Library, hash: &[u8; 32]) -> Result<Option<Vec<u8>>, Failure> {
    let Some(size) = library.size(hash).map_err(failure)? else { return Ok(None) };
    if size > EVENT_OBJECT_BYTES {
        return Err(failure("an event's object is past what is read whole"));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    while (bytes.len() as u64) < size {
        let length = u32::try_from((size - bytes.len() as u64).min(1 << 20)).expect("a mebibyte");
        let piece = library.read(hash, bytes.len() as u64, length).map_err(failure)?;
        if piece.is_empty() {
            return Err(failure("an event's object ended before its length"));
        }
        bytes.extend_from_slice(&piece);
    }
    Ok(Some(bytes))
}

/// The event an entry of the timeline holds: itself, or the object it is
/// kept whole in; none where that object is not here.
pub(super) fn event_of(library: &Library, entry: &Entry) -> Result<Option<SessionEvent>, Failure> {
    match entry.body.split_first() {
        Some((&EVENT, encoded)) => Ok(Some(serde_json::from_slice(encoded).map_err(failure)?)),
        Some((&OVERSIZED, _)) => {
            let Some(object) = entry.objects.first() else { return Ok(None) };
            // One past what is read whole is passed by, as one not here.
            if library.size(&object.hash).map_err(failure)?.is_some_and(|size| size > EVENT_OBJECT_BYTES) {
                return Ok(None);
            }
            let Some(bytes) = object_whole(library, &object.hash)? else { return Ok(None) };
            Ok(Some(serde_json::from_slice(&bytes).map_err(failure)?))
        }
        _ => Ok(None),
    }
}

/// The events of records stopped short that a writing leaves as the
/// timeline has them (`Span::Added`, one set of `frozen` keys for each
/// record): of `events`, each with its identity, what `names::frozen` knows
/// it by and whether it is named (`names::key`), those whose items the
/// timeline has in any state (`had`, asked of some identities) — and one
/// known only by its moment where the timeline has an item of its record
/// that is named. A moment more than one record knows is no one record's:
/// it is judged by its own item alone. Decided whole, before anything is
/// written.
pub(super) fn untouched(
    frozen: &[HashSet<String>],
    events: &mut dyn Iterator<Item = (Vec<u8>, String, bool)>,
    had: &mut HadAny<'_>,
) -> Result<HashSet<Vec<u8>>, Failure> {
    let shared: HashSet<&String> = {
        let mut seen = HashSet::new();
        frozen.iter().flatten().filter(|key| !seen.insert(*key)).collect()
    };
    // Each record's events, those named and those known by their moment:
    // whether the timeline has a record's is that record's.
    type Identities = Vec<Vec<u8>>;
    let mut alone: Identities = Vec::new();
    let mut records: Vec<(Identities, Identities)> = frozen.iter().map(|_| (Vec::new(), Vec::new())).collect();
    for (identity, known, named) in events {
        for (kept, (record_named, record_moments)) in frozen.iter().zip(&mut records) {
            if kept.contains(&known) {
                if named {
                    record_named.push(identity.clone());
                } else if shared.contains(&known) {
                    alone.push(identity.clone());
                } else {
                    record_moments.push(identity.clone());
                }
            }
        }
    }
    let asked: Vec<Vec<u8>> = records.iter().flat_map(|(named, moments)| named.iter().chain(moments)).chain(&alone).cloned().collect();
    let mut found = had(&asked)?;
    for (named, moments) in records {
        if named.iter().any(|identity| found.contains(identity)) {
            found.extend(moments);
        }
    }
    Ok(found)
}

/// Which of some identities the timeline's index has a row of, in any state.
pub(super) type HadAny<'a> = dyn FnMut(&[Vec<u8>]) -> Result<HashSet<Vec<u8>>, Failure> + 'a;

/// The holding period the timeline's scope is in, the scope begun and
/// proposed if it is not here yet; refused where it is not `expected`, the
/// period the window was read on from (GG02).
fn holding(library: &Library, placed: &Placed, expected: Option<i64>) -> Result<i64, Failure> {
    // Begun under the term the machine was on the network when it was
    // placed: after leaving it, nothing is begun again.
    let holding = match library.holding(&placed.scope).map_err(failure)? {
        Some(holding) => holding,
        None => {
            let proposal = serde_json::to_vec(&placed.proposal).map_err(failure)?;
            library.propose_on(&placed.network, &placed.scope, proposal).map_err(failure)?;
            library.holding(&placed.scope).map_err(failure)?.ok_or_else(|| failure("the timeline's scope is not held here"))?
        }
    };
    if expected.is_some_and(|expected| expected != holding) {
        return Err(failure("the timeline's scope was held anew since it was read; it is made again whole"));
    }
    Ok(holding)
}

/// How long each part of the writes kept on disk took, for the bench: by
/// part, seconds in all.
#[cfg(feature = "bench")]
pub(crate) static PHASES: std::sync::Mutex<Vec<(&'static str, f64)>> = std::sync::Mutex::new(Vec::new());

/// The most resident memory the bench's sampler saw since the last part
/// ended (`phase`), and the most of each part, for the bench.
#[cfg(feature = "bench")]
pub(crate) static PHASE_PEAK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "bench")]
pub(crate) static PEAKS: std::sync::Mutex<Vec<(&'static str, u64)>> = std::sync::Mutex::new(Vec::new());

/// Begins counting the parts afresh, for the bench.
#[cfg(feature = "bench")]
pub(crate) fn begin_phases() {
    PHASES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    PEAKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    PHASE_PEAK.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Counts the time since `since` to `part` (`PHASES`), and starts again.
pub(super) fn phase(part: &'static str, since: &mut std::time::Instant) {
    #[cfg(feature = "bench")]
    {
        let took = since.elapsed().as_secs_f64();
        let mut phases = PHASES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match phases.iter_mut().find(|(name, _)| *name == part) {
            Some((_, all)) => *all += took,
            None => phases.push((part, took)),
        }
        let peak = PHASE_PEAK.swap(0, std::sync::atomic::Ordering::Relaxed);
        let mut peaks = PEAKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match peaks.iter_mut().find(|(name, _)| *name == part) {
            Some((_, most)) => *most = (*most).max(peak),
            None => peaks.push((part, peak)),
        }
    }
    #[cfg(not(feature = "bench"))]
    let _ = part;
    *since = std::time::Instant::now();
}

/// What a write of a whole history kept on disk left (`write_kept`): how
/// many items it wrote and how many went, the holding period, and of the
/// items it placed, the highest order key of those settled (those held in
/// the window told to `held` as they come, none kept here).
pub(super) struct WroteKept {
    pub(super) written: usize,
    pub(super) gone: usize,
    pub(super) holding: i64,
    pub(super) floor: u64,
}

/// What a writing kept on disk asks of each item it places (`write_kept`):
/// given its identity, time and order key, whether it is held in the window.
pub(super) type HeldIn<'a> = dyn FnMut(&[u8], jiff::Timestamp, u64) -> Result<bool, Failure> + 'a;

/// How much of the timeline a writing kept on disk (`write_kept`) reaches,
/// as `Span` says of one held in memory: `Whole`, `Kept`, or `Added`, the
/// sets of what each record stopped short knows its events by kept in the
/// work store with the final events (`Spill::push_known`); or `From`, the
/// window from `since` on, as `Span::From` writes it — the items the window
/// reads again or takes away kept in the store (`Spill::push_before`), each
/// of its events placed past `floor`, an event from before it the timeline
/// has keeping its place but those told again at every reading (`again`),
/// written only in the holding period `expected` where one is given.
pub(super) enum Reach<'a> {
    Whole,
    Kept,
    Added,
    From { since: jiff::Timestamp, floor: u64, again: &'a HashSet<Vec<u8>>, expected: Option<i64> },
}

/// `write` of a whole history whose final events are kept in `spill` with
/// their identities (design §28.51), over `reach`: the timeline's items
/// read into it by identity, order key and the length of their bodies; all
/// that is decided before anything is written — every event's key given as
/// `write` gives it, those left as the timeline has them (`Added`,
/// `untouched`, decided in the store), the items that go (`Whole`) — then the
/// items no event is written gone, then the events taken a batch at a time
/// with the bodies the timeline has of them, compared and written as `write`
/// does. What stays in memory is each event's order key. Each item it
/// places is offered to `held` once, in order, with its identity, time and
/// key: whether it is held in the window — which `held` keeps where it
/// will — and of the rest the highest key is told.
#[allow(clippy::too_many_arguments)]
pub(super) fn write_kept(
    library: &Library,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    resident: &ResidentId,
    placed: &Placed,
    spill: &mut super::spill::Spill,
    early: Option<&Early>,
    reach: Reach<'_>,
    held: &mut HeldIn<'_>,
) -> Result<WroteKept, Failure> {
    let _writing = Writing::take(resident);
    // What was offered is put still; nothing more is.
    if let Some(early) = early {
        early.close();
    }
    let mut since = std::time::Instant::now();
    let expected = if let Reach::From { expected, .. } = &reach { *expected } else { None };
    let holding = holding(library, placed, expected)?;
    let kept = |error: String| Failure::new(error);
    if let Reach::From { .. } = reach {
        // A window: the timeline asked of its events and of what it reads
        // again or takes away alone, as `write` asks of them.
        let mut after: Option<Vec<u8>> = None;
        loop {
            let page = spill.asked_from(after.as_deref(), PAGE).map_err(kept)?;
            let Some(last) = page.last().cloned() else { break };
            after = Some(last);
            for (item, order, length) in library.orders_of(&placed.scope, Timeline::STREAM, &page).map_err(failure)? {
                spill.push_had(item, order, length).map_err(kept)?;
            }
            if page.len() < PAGE {
                break;
            }
        }
    } else {
        let mut after = None;
        loop {
            let page = library.orders(&placed.scope, Timeline::STREAM, after.clone(), PAGE).map_err(failure)?;
            let Some((item, order, _)) = page.last() else { break };
            after = Some((order.clone(), item.clone()));
            let full = page.len() == PAGE;
            for (item, order, length) in page {
                spill.push_had(item, order, length).map_err(kept)?;
            }
            if !full {
                break;
            }
        }
    }
    phase("had", &mut since);
    // For each final event: whether the timeline has it live, its key there,
    // whether it is left as it is, and of a window whether it is from before
    // it — a key and a byte each.
    let early_of = if let Reach::From { since, again, .. } = &reach { Some((*since, *again)) } else { None };
    let mut keyed = spill.had_keys(early_of).map_err(kept)?;
    phase("had keys", &mut since);
    // Added: the events of a record stopped short whose items the timeline
    // has are not written, decided whole first (`untouched`): the timeline
    // asked of those the records know a page at a time, what it has kept
    // in the store, and the rest decided there.
    if let Reach::Added = reach {
        let mut after = 0;
        loop {
            let page = spill.known_from(after).map_err(kept)?;
            let Some(&(last, _)) = page.last() else { break };
            after = last;
            let asked: Vec<Vec<u8>> = page.into_iter().map(|(_, identity)| identity).collect();
            let found = library.had_items(&placed.scope, Timeline::STREAM, &asked).map_err(failure)?;
            spill.push_found(&found).map_err(kept)?;
        }
        phase("asked what it had", &mut since);
        spill.left(&mut |place| keyed.leave(place)).map_err(kept)?;
        phase("left", &mut since);
    }
    let keys = if let Reach::From { floor, .. } = &reach {
        window_keys(&keyed, *floor)
    } else {
        let mut keys = order::assign_with(keyed.len(), &|index| keyed.key(index));
        // Not written, an event left as the timeline has it keeps the key it
        // has: placed between two of them without room, a new one may share
        // a key with the next (as `write` places it).
        order::within_with(&mut keys, &|index| if keyed.left(index) { keyed.key(index) } else { None });
        keys
    };
    keyed.keys_made();
    phase("keys", &mut since);
    let writer = Writer::<Timeline>::new(library, &placed.scope, Timeline::STREAM).map_err(failure)?.in_holding(holding);
    let mut pending = Pending { library, writer: &writer, signer, writes: Vec::new(), leases: Vec::new(), bytes: 0 };
    let mut gone = 0;
    let mut last: Option<Vec<u8>> = None;
    // Only a reading of all of every record takes anything away — or of a
    // window, what it reads again that its history no longer has.
    if matches!(reach, Reach::Whole | Reach::From { .. }) {
        let window = matches!(reach, Reach::From { .. });
        loop {
            let page = if window { spill.gone_before(last.as_deref(), WRITES_AT_ONCE) } else { spill.gone(last.as_deref(), WRITES_AT_ONCE) }.map_err(kept)?;
            let Some((identity, _)) = page.last() else { break };
            last = Some(identity.clone());
            for (identity, order) in page {
                gone += 1;
                pending.push(Write { item: identity, order, value: None, objects: Vec::new() }, None, 0)?;
            }
        }
    }
    phase("gone", &mut since);
    let mut floor = 0u64;
    let written = lined(library, resident, &mut pending, |lining| {
        let mut written = 0;
        let mut from = 0;
        loop {
            let finals = spill.finals(from, WRITES_AT_ONCE, BYTES_AT_ONCE as u64).map_err(kept)?;
            phase("finals read", &mut since);
            if finals.is_empty() {
                break;
            }
            let taken = finals.len();
            let asked: Vec<Vec<u8>> = finals.iter().filter(|kept| kept.had.is_some()).map(|kept| kept.identity.clone()).collect();
            let had = if asked.is_empty() { HashMap::new() } else { library.items(&placed.scope, Timeline::STREAM, &asked).map_err(failure)? };
            phase("had bodies", &mut since);
            for (offset, kept) in finals.into_iter().enumerate() {
                let (key, left) = (keys[from + offset], keyed.left(from + offset));
                // A tombstone left as it is is not among the window's items.
                // Nor one from before a window, which keeps its place.
                if (!left || keyed.live(from + offset)) && !keyed.early(from + offset) && !held(&kept.identity, kept.at, key)? {
                    floor = floor.max(key);
                }
                if left {
                    continue;
                }
                let order = key.to_be_bytes().to_vec();
                // Its record as made from the event itself (`finals`).
                let body = kept.record;
                let objects: Vec<fastctx_content::ObjectRef> = kept.object.iter().map(|(object, _)| *object).collect();
                if let Some(entry) = had.get(&kept.identity)
                    && entry.order == order
                    && entry.body == body
                    && entry.objects == objects
                {
                    continue;
                }
                // Put already as it was read, its lease is taken.
                let leased = objects.first().and_then(|object| early.and_then(|early| early.take(object)));
                let put = if leased.is_some() { None } else { kept.object.map(|(object, encoded)| (encoded, object)) };
                let bytes = body.len();
                phase("bodies", &mut since);
                lining.push(Queued { write: Write { item: kept.identity, order, value: Some(body), objects }, put, leased, bytes })?;
                phase("waiting on puts and commits", &mut since);
                written += 1;
            }
            phase("bodies", &mut since);
            from += taken;
        }
        Ok(written)
    })?;
    pending.flush()?;
    phase("waiting on puts and commits", &mut since);
    Ok(WroteKept { written, gone, holding, floor })
}

/// The keys of the final events of a window (`Reach::From`), as `write`
/// gives them: those from before it keep the keys they have; the others,
/// the window's, are keyed past `floor` (`order::assign` of them, `floor`
/// first, each key they had above it kept where it can be). Only the
/// window's places are held while it decides, then their keys spread out
/// over all in place.
fn window_keys(keyed: &super::spill::Keyed, floor: u64) -> Vec<u64> {
    let window: Vec<u32> = (0..keyed.len()).filter(|index| !keyed.early(*index)).map(|index| index as u32).collect();
    let mut keys =
        order::assign_with(window.len() + 1, &|place| if place == 0 { Some(floor) } else { keyed.key(window[place - 1] as usize).filter(|key| *key > floor) });
    let mut next = window.len();
    drop(window);
    // From the last back: a window's event never stands before where its
    // key was assigned, so none is overwritten before it is moved. Grown
    // to its length exactly, not doubled.
    keys.reserve_exact((keyed.len() + 1).saturating_sub(keys.len()));
    keys.resize(keyed.len() + 1, 0);
    for index in (0..keyed.len()).rev() {
        keys[index + 1] = if keyed.early(index) {
            keyed.key(index).unwrap_or(0)
        } else {
            next -= 1;
            keys[next + 1]
        };
    }
    keys.remove(0);
    keys
}

/// The record of an event small enough for one: itself (`body`).
fn body_small(encoded: &[u8]) -> (Vec<u8>, Option<fastctx_content::ObjectRef>) {
    let mut body = Vec::with_capacity(encoded.len() + 1);
    body.push(EVENT);
    body.extend_from_slice(encoded);
    (body, None)
}

/// An event as its record holds it: itself, or when it is too large for a
/// record, what is shown in its place, with the object the whole of it is
/// (put only when the record is written).
pub(super) fn body(event: &SessionEvent, encoded: &[u8]) -> Result<(Vec<u8>, Option<fastctx_content::ObjectRef>), Failure> {
    if encoded.len() < RECORD_BYTES {
        return Ok(body_small(encoded));
    }
    let hash: [u8; 32] = Sha256::digest(encoded).into();
    let object = fastctx_content::ObjectRef { hash, size: encoded.len() as u64 };
    let mut body = vec![OVERSIZED];
    body.extend_from_slice(
        &serde_json::to_vec(&serde_json::json!({
            "bytes": encoded.len(),
            "slim": preview::preview(event),
            "likeness": super::identity::likeness_of(event, &hash),
        }))
        .map_err(failure)?,
    );
    Ok((body, Some(object)))
}

/// Writes on their way to the timeline, a bounded commit at a time. The
/// timeline is made again from its sources, so no commit waits for the disk.
struct Pending<'a> {
    library: &'a Library,
    writer: &'a Writer<Timeline>,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    writes: Vec<Write<Timeline>>,
    leases: Vec<fastctx_content::v2::Lease>,
    bytes: usize,
}

impl Pending<'_> {
    fn push(&mut self, write: Write<Timeline>, lease: Option<fastctx_content::v2::Lease>, bytes: usize) -> Result<(), Failure> {
        self.bytes += 128 + bytes;
        self.writes.push(write);
        self.leases.extend(lease);
        if self.writes.len() >= WRITES_AT_ONCE || self.bytes >= BYTES_AT_ONCE {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Failure> {
        if self.writes.is_empty() {
            return Ok(());
        }
        let (writes, leases) = (std::mem::take(&mut self.writes), std::mem::take(&mut self.leases));
        self.bytes = 0;
        // Not written, its objects' leases are let go rather than holding
        // the objects until they run out; written after all, the records
        // refer to them and the leases are not needed.
        let held = leases.clone();
        if let Err(error) = self.writer.write_all(writes, leases, false, self.signer.clone()) {
            for lease in held {
                let _ = self.library.release(lease);
            }
            return Err(failure(error));
        }
        Ok(())
    }
}

impl Drop for Pending<'_> {
    /// Writes never committed — a writing given up before its last commit,
    /// however it failed — leave their objects' leases let go, not holding
    /// the objects until they run out.
    fn drop(&mut self) {
        for lease in self.leases.drain(..) {
            let _ = self.library.release(lease);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use fastctx_types::session::{Happened, Health, HealthKind};
    use fastctx_types::session::UserMessage;

    pub(in crate::timeline) struct Device;
    impl fastctx_content::v2::Signer for Device {
        fn device(&self) -> [u8; 32] {
            [1; 32]
        }
        fn key(&self) -> [u8; 32] {
            [2; 32]
        }
        fn sign(&self, header: &[u8]) -> Vec<u8> {
            header.to_vec()
        }
    }

    fn at(seconds: i64) -> jiff::Timestamp {
        jiff::Timestamp::from_second(1_800_000_000 + seconds).unwrap()
    }

    pub(in crate::timeline) fn words(id: &str, text: &str, seconds: i64) -> SessionEvent {
        let message = UserMessage {
            attachments: Vec::new(),
            images: Vec::new(),
            id: id.into(),
            client_id: None,
            content: Vec::new(),
            hook_fragments: Vec::new(),
            presentation: serde_json::Value::Null,
            detail: serde_json::Value::Null,
        };
        SessionEvent {
            at: at(seconds),
            happened: Happened::Heard { delivery: None, turn: None, from: None, text: text.into(), message: Some(Box::new(message)), notice: None },
        }
    }

    /// An event known only by its moment.
    pub(in crate::timeline) fn note(text: &str, seconds: i64) -> SessionEvent {
        SessionEvent { at: at(seconds), happened: Happened::Health { turn: None, health: Health { kind: HealthKind::Warning, text: text.into() } } }
    }

    /// A record stopped short (`Span::Added`) writes only what the timeline
    /// has no item of: an item it has is not written over, a tombstone not
    /// brought back, an event known by its moment not added beside the one
    /// made from all of the record; each left keeps its order key.
    #[test]
    fn a_record_stopped_short_leaves_what_the_timeline_has() {
        let dir = std::env::temp_dir().join(format!("fastctx-timeline-added-{}", std::process::id()));
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
        fastctx_platform::disk::create_dir_all(&dir).unwrap();
        let (library, _) = Library::open(&dir).unwrap();
        library.know(Arc::new(fastctx_content::v2::Kinds::new().register::<Timeline>()));
        library.install("s", b"d".to_vec(), 1).unwrap();
        let hub = HubId::from_bytes(*b"testhub0");
        let network = library.join(&hub.to_string(), &[1; 32], false).unwrap();
        let readers = Readers::Conversation { openers: Vec::new(), born_on: None };
        let proposal = crate::assets::Proposal { hub: hub.to_string(), readers, classes: Vec::new() };
        let placed = Placed { scope: "s".into(), hub, proposal, network };
        let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
        let resident: ResidentId = "claude-a0a0".parse().unwrap();
        let none = HashSet::new();
        let whole = vec![words("a", "first", 1), words("b", "second", 2), note("noted", 3), words("f", "fourth", 4)];
        write(&library, signer.clone(), &resident, &placed, &whole, Span::Whole, &none, None).unwrap();
        // B taken back since, by a reading of all.
        let taken = vec![whole[0].clone(), whole[2].clone(), whole[3].clone()];
        assert_eq!(write(&library, signer.clone(), &resident, &placed, &taken, Span::Whole, &none, None).unwrap().gone, 1);
        let before = held(&library, "s").unwrap();
        assert_eq!(before.len(), 3);
        // The record stopped short: A shows less, B is there again, the note
        // made at another moment from less of it, N new.
        let short = vec![words("a", "fir", 1), words("b", "second", 2), note("noted from less", 5), words("n", "new", 6), words("f", "fourth", 4)];
        let frozen: HashSet<String> = short.iter().map(super::super::names::frozen).collect();
        let wrote = write(&library, signer.clone(), &resident, &placed, &short, Span::Added(std::slice::from_ref(&frozen)), &none, None).unwrap();
        assert_eq!((wrote.written, wrote.gone), (1, 0), "N alone written");
        let after = held(&library, "s").unwrap();
        assert_eq!(after.len(), 4, "A, the note, F and N; B still gone");
        for (item, entry) in &before {
            assert_eq!(after.get(item).map(|entry| (&entry.order, &entry.body)), Some((&entry.order, &entry.body)), "left as it was");
        }
        let identities = super::super::names::identities(&short);
        assert!(!after.contains_key(&identities[1]), "the tombstone not brought back");
        assert!(!after.contains_key(&identities[2]), "the note from less not added beside the one it had");
        assert!(after.contains_key(&identities[3]), "N written");
        // Told with the keys the items have: the tombstone and the note left
        // out are not among them.
        let told: HashMap<&Vec<u8>, u64> = wrote.items.iter().map(|(identity, _, key)| (identity, *key)).collect();
        assert_eq!(told.len(), 3, "{told:?}");
        for index in [0, 4] {
            let had = u64::from_be_bytes(after[&identities[index]].order.as_slice().try_into().unwrap());
            assert_eq!(told.get(&identities[index]), Some(&had));
        }
        drop(library);
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }

    /// A host's call of World's `run` finished, of `id`.
    fn ran(id: &str, seconds: i64) -> SessionEvent {
        let tool = fastctx_types::session::Tool {
            run: None,
            name: "run".into(),
            server: Some("fastctx".into()),
            machine: None,
            does: fastctx_types::session::Does::Command { command: "true".into(), cwd: None, exit: None, job: None },
            input: serde_json::Value::Null,
            output: Vec::new(),
            status: fastctx_types::session::ToolStatus::Done,
            duration_ms: None,
        };
        let item = fastctx_types::session::Item { id: id.into(), draft: None, started_at: None, detail: None, what: fastctx_types::session::What::Tool(Box::new(tool)) };
        SessionEvent { at: at(seconds), happened: Happened::Finished { turn: "t".into(), item } }
    }

    /// A host's item finished, a call of `id`, its text `text`.
    fn finished(id: &str, text: &str, seconds: i64) -> SessionEvent {
        let item = fastctx_types::session::Item {
            id: id.into(),
            draft: None,
            started_at: None,
            detail: None,
            what: fastctx_types::session::What::Message { text: text.into() },
        };
        SessionEvent { at: at(seconds), happened: Happened::Finished { turn: "t".into(), item } }
    }

    /// The items of calls whose runs' facts changed are given their facts
    /// again where they stand before the window, each keeping its place, a
    /// large one's object put anew; one past the floor is the window's, one
    /// not in the timeline is nothing; given again, nothing changes.
    #[test]
    fn the_items_before_the_window_are_given_their_runs_facts_again() {
        let dir = std::env::temp_dir().join(format!("fastctx-timeline-runs-{}", std::process::id()));
        let (library, placed) = library(&dir);
        let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
        let resident: ResidentId = "claude-a0a0".parse().unwrap();
        let history = vec![
            finished("c1", "one", 1),
            finished("c2", "two", 2),
            finished("c3", &"large ".repeat(20_000), 3),
            words("w", "said", 4),
            finished("c4", "four", 5),
        ];
        write(&library, signer.clone(), &resident, &placed, &history, Span::Whole, &HashSet::new(), None).unwrap();
        let identities = super::super::names::identities(&history);
        let before = held(&library, "s").unwrap();
        let key = |entry: &Entry| u64::from_be_bytes(entry.order.as_slice().try_into().unwrap());
        let floor = key(&before[&identities[2]]);
        assert!(key(&before[&identities[4]]) > floor);
        let changed: HashSet<&str> = ["c1", "c3", "c4", "c9"].into_iter().collect();
        let attach = |event: &mut SessionEvent| {
            if let Happened::Finished { item, .. } = &mut event.happened
                && changed.contains(item.id.as_str())
            {
                item.detail = Some(serde_json::json!({ "exit": 7 }));
            }
            Ok(())
        };
        let calls: Vec<String> = changed.iter().map(|call| call.to_string()).collect();
        let written = refresh_runs(&library, signer.clone(), &resident, &placed, &attach, &calls, floor, None).unwrap();
        assert_eq!(written, 2, "c1 and c3; c4 is the window's, c9 is not there");
        let after = held(&library, "s").unwrap();
        assert_eq!(after.len(), before.len());
        for (index, refreshed) in [(0, true), (1, false), (2, true), (3, false), (4, false)] {
            let (was, is) = (&before[&identities[index]], &after[&identities[index]]);
            assert_eq!(was.order, is.order, "{index} keeps its place");
            let event = event_of(&library, is).unwrap().expect("its event");
            match &event.happened {
                Happened::Finished { item, .. } => assert_eq!(item.detail.is_some(), refreshed, "{index}"),
                _ => assert!(!refreshed),
            }
        }
        assert_eq!(after[&identities[2]].objects.len(), 1, "the large one still an object");
        assert_ne!(after[&identities[2]].objects, before[&identities[2]].objects, "put anew");
        assert_eq!(refresh_runs(&library, signer, &resident, &placed, &attach, &calls, floor, None).unwrap(), 0);
        drop(library);
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }

    /// A call alike in many records makes as many items, every one of them
    /// given its facts again; and where what changed is not known call by
    /// call, every item before the window that can tell of a run is, a page
    /// at a time, and none past it.
    #[test]
    fn every_item_of_a_run_is_given_its_facts_again() {
        let dir = std::env::temp_dir().join(format!("fastctx-timeline-runs-all-{}", std::process::id()));
        let (library, placed) = library(&dir);
        let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
        let resident: ResidentId = "claude-a0a0".parse().unwrap();
        let mut history: Vec<SessionEvent> = (0..10).map(|at| ran("c9", at)).collect();
        history.extend([ran("c1", 20), finished("c2", "run", 21), words("w", "said", 22), ran("c4", 23)]);
        write(&library, signer.clone(), &resident, &placed, &history, Span::Whole, &HashSet::new(), None).unwrap();
        let identities = super::super::names::identities(&history);
        let key = |entry: &Entry| u64::from_be_bytes(entry.order.as_slice().try_into().unwrap());
        let floor = key(&held(&library, "s").unwrap()[&identities[11]]);
        let attach = |event: &mut SessionEvent| {
            if let Happened::Finished { item, .. } = &mut event.happened {
                item.detail = Some(serde_json::json!({ "exit": 1 }));
            }
            Ok(())
        };
        let calls = vec!["c9".to_string()];
        assert_eq!(refresh_runs(&library, signer.clone(), &resident, &placed, &attach, &calls, floor, None).unwrap(), 10, "all ten of c9");
        assert_eq!(refresh_every_run(&library, signer.clone(), &resident, &placed, &attach, floor, None).unwrap(), 1, "c1: c2 is no run, c4 is past the floor, c9 had its facts");
        let after = held(&library, "s").unwrap();
        let detail = |index: usize| match event_of(&library, &after[&identities[index]]).unwrap().unwrap().happened {
            Happened::Finished { item, .. } => item.detail.is_some(),
            _ => false,
        };
        assert!((0..11).all(detail));
        assert!(!detail(11) && !detail(13), "c2 no run, c4 past the floor");
        assert_eq!(refresh_every_run(&library, signer, &resident, &placed, &attach, floor, None).unwrap(), 0);
        drop(library);
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }

    /// A small, fixed-seed source of choices.
    pub(in crate::timeline) struct Choices(pub(in crate::timeline) u64);
    impl Choices {
        pub(in crate::timeline) fn pick(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    /// A library with the scope held, and where it goes.
    pub(in crate::timeline) fn library(dir: &std::path::Path) -> (Library, Placed) {
        let _ = fastctx_platform::disk::remove_dir_all(dir);
        fastctx_platform::disk::create_dir_all(dir).unwrap();
        let (library, _) = Library::open(dir).unwrap();
        library.know(Arc::new(fastctx_content::v2::Kinds::new().register::<Timeline>()));
        library.install("s", b"d".to_vec(), 1).unwrap();
        let hub = HubId::from_bytes(*b"testhub0");
        let network = library.join(&hub.to_string(), &[1; 32], false).unwrap();
        let readers = Readers::Conversation { openers: Vec::new(), born_on: None };
        let proposal = crate::assets::Proposal { hub: hub.to_string(), readers, classes: Vec::new() };
        (library, Placed { scope: "s".into(), hub, proposal, network })
    }

    /// A history of words and notes alike often: names and moments that
    /// meet, large words now and then.
    fn history(choices: &mut Choices) -> Vec<SessionEvent> {
        (0..choices.pick(12))
            .map(|_| {
                let seconds = choices.pick(8) as i64;
                match choices.pick(4) {
                    0 => note(["a", "b"][choices.pick(2) as usize], seconds),
                    1 => words(&format!("m{}", choices.pick(6)), &"long ".repeat(20_000), seconds),
                    _ => words(&format!("m{}", choices.pick(6)), &format!("w{}", choices.pick(3)), seconds),
                }
            })
            .collect()
    }

    /// A live item of the timeline: identity, order, body and objects.
    type Item = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<fastctx_content::ObjectRef>);

    /// Every live item of the timeline.
    pub(in crate::timeline) fn all(library: &Library) -> Vec<Item> {
        let mut items: Vec<_> = held(library, "s").unwrap().into_values().map(|entry| (entry.item, entry.order, entry.body, entry.objects)).collect();
        items.sort_by(|one, other| one.0.cmp(&other.0));
        items
    }

    /// A whole writing kept on disk (`write_kept`) leaves the timeline as one
    /// held in memory (`write`) does, over `Whole`, `Kept` and `Added`, on a
    /// timeline that had items, some of them gone: the same items, keys,
    /// bodies and objects, the same counts, the same highest key.
    #[test]
    fn a_writing_kept_on_disk_is_the_writing_held_in_memory() {
        let base = std::env::temp_dir().join(format!("fastctx-timeline-kept-{}", std::process::id()));
        let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
        let resident: ResidentId = "claude-a0a0".parse().unwrap();
        let none = HashSet::new();
        let mut choices = Choices(0x2545_f491_4f6c_dd1d);
        for round in 0..40 {
            let (held_library, held_placed) = library(&base.join(format!("held-{round}")));
            let (kept_library, kept_placed) = library(&base.join(format!("kept-{round}")));
            // What the timeline had: a history, then less of it.
            let first = history(&mut choices);
            let less: Vec<SessionEvent> = first.iter().filter(|_| choices.pick(3) != 0).cloned().collect();
            for (library, placed) in [(&held_library, &held_placed), (&kept_library, &kept_placed)] {
                write(library, signer.clone(), &resident, placed, &first, Span::Whole, &none, None).unwrap();
                write(library, signer.clone(), &resident, placed, &less, Span::Whole, &none, None).unwrap();
            }
            let now = history(&mut choices);
            let known: Vec<String> = now.iter().map(super::super::names::frozen).collect();
            let frozen: Vec<HashSet<String>> =
                (0..1 + choices.pick(2)).map(|_| known.iter().filter(|_| choices.pick(2) == 0).cloned().collect()).collect();
            let span = choices.pick(3);
            let pick = |span| match span {
                0 => Span::Whole,
                1 => Span::Kept,
                _ => Span::Added(&frozen),
            };
            let wrote = write(&held_library, signer.clone(), &resident, &held_placed, &now, pick(span), &none, None).unwrap();
            let spill_root = base.join(format!("spill-{round}"));
            let mut spill = super::super::spill::Spill::new(spill_root.clone(), "claude-a0a0");
            spill.begin().unwrap();
            let identities = super::super::names::identities(&now);
            for (event, identity) in now.iter().zip(&identities) {
                let encoded = serde_json::to_vec(event).unwrap();
                let (record, object) = body(event, &encoded).unwrap();
                let frozen_key = (super::super::names::frozen(event), super::super::names::key(event).is_some());
                spill.push_final(identity.as_slice().try_into().unwrap(), event.at, record, object.map(|object| (object, encoded)), Some(frozen_key)).unwrap();
            }
            for (record, set) in frozen.iter().enumerate() {
                for key in set {
                    spill.push_known(record as u32, key.clone()).unwrap();
                }
            }
            let reach = match span {
                0 => Reach::Whole,
                1 => Reach::Kept,
                _ => Reach::Added,
            };
            let kept = write_kept(&kept_library, signer.clone(), &resident, &kept_placed, &mut spill, None, reach, &mut |_, _, _| Ok(false)).unwrap();
            let floor = wrote.items.iter().map(|(_, _, key)| *key).max().unwrap_or(0);
            assert_eq!((kept.written, kept.gone, kept.floor), (wrote.written, wrote.gone, floor), "round {round}, span {span}");
            assert_eq!(all(&kept_library), all(&held_library), "round {round}, span {span}");
            drop(spill);
            let _ = super::super::spill::clear(&spill_root);
        }
        let _ = fastctx_platform::disk::remove_dir_all(&base);
    }

    /// A window written from a reading kept on disk (`write_kept`,
    /// `Reach::From`) leaves the timeline as one held in memory (`write`,
    /// `Span::From`) does: past a floor, reading again the items past it and
    /// others told before (some below it), the events from before the window
    /// the timeline has keeping their places — but those told again — and
    /// their bodies still brought up to date; in the holding period it was
    /// read in, or none. The same items, keys, bodies and objects, the same
    /// counts, the same items held in the window and settled below it.
    #[test]
    fn a_window_written_from_disk_is_the_window_held_in_memory() {
        let base = std::env::temp_dir().join(format!("fastctx-timeline-window-{}", std::process::id()));
        let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
        let resident: ResidentId = "claude-a0a0".parse().unwrap();
        let none = HashSet::new();
        let mut choices = Choices(0x3c6e_f372_fe94_f82b);
        // How often: early events kept in place, items taken away, an event
        // told again placed in the window, an item held in the window.
        let mut met = [0usize; 4];
        for round in 0..120 {
            let (held_library, held_placed) = library(&base.join(format!("held-{round}")));
            let (kept_library, kept_placed) = library(&base.join(format!("kept-{round}")));
            let first = history(&mut choices);
            let less: Vec<SessionEvent> = first.iter().filter(|_| choices.pick(3) != 0).cloned().collect();
            for (library, placed) in [(&held_library, &held_placed), (&kept_library, &kept_placed)] {
                write(library, signer.clone(), &resident, placed, &first, Span::Whole, &none, None).unwrap();
                write(library, signer.clone(), &resident, placed, &less, Span::Whole, &none, None).unwrap();
            }
            // Where the last reading left it: a floor among the keys there,
            // the items past it, and some told then wherever they stand.
            let had = all(&held_library);
            let keys: Vec<u64> = had.iter().filter_map(|(_, order, ..)| order.as_slice().try_into().ok().map(u64::from_be_bytes)).collect();
            let floor = if keys.is_empty() || choices.pick(4) == 0 { 0 } else { keys[choices.pick(keys.len() as u64) as usize] };
            let mut before: Vec<Vec<u8>> = had
                .iter()
                .filter(|(_, order, ..)| order.as_slice().try_into().ok().map(u64::from_be_bytes).is_some_and(|key| key > floor))
                .map(|(identity, ..)| identity.clone())
                .collect();
            for (identity, ..) in &had {
                if choices.pick(4) == 0 && !before.contains(identity) {
                    before.push(identity.clone());
                }
            }
            let now = history(&mut choices);
            let identities = super::super::names::identities(&now);
            let since = at(choices.pick(9) as i64);
            let again: HashSet<Vec<u8>> = identities.iter().filter(|_| choices.pick(5) == 0).cloned().collect();
            let expected = if choices.pick(2) == 0 { held_library.holding("s").unwrap() } else { None };
            let settled = at(choices.pick(9) as i64);
            let held_at = |identity: &[u8], at: jiff::Timestamp| at >= settled || at < since || again.contains(identity);
            let timed: Vec<(Vec<u8>, jiff::Timestamp)> = before.iter().map(|identity| (identity.clone(), since)).collect();
            let wrote = write(&held_library, signer.clone(), &resident, &held_placed, &now, Span::From { since, before: &timed, floor }, &again, expected).unwrap();

            let spill_root = base.join(format!("spill-{round}"));
            let mut spill = super::super::spill::Spill::new(spill_root.clone(), "claude-a0a0");
            spill.begin().unwrap();
            for (event, identity) in now.iter().zip(&identities) {
                let encoded = serde_json::to_vec(event).unwrap();
                let (record, object) = body(event, &encoded).unwrap();
                spill.push_final(identity.as_slice().try_into().unwrap(), event.at, record, object.map(|object| (object, encoded)), None).unwrap();
            }
            for identity in &before {
                spill.push_before(identity.clone()).unwrap();
            }
            let mut kept_held = Vec::new();
            let kept = write_kept(&kept_library, signer.clone(), &resident, &kept_placed, &mut spill, None, Reach::From { since, floor, again: &again, expected }, &mut |identity, at, key| {
                let held = held_at(identity, at);
                if held {
                    kept_held.push((identity.to_vec(), at, key));
                }
                Ok(held)
            })
            .unwrap();
            // As `tail::place` settles and holds the window's items.
            let settled_floor = wrote.items.iter().filter(|(identity, at, _)| !held_at(identity, *at)).map(|(_, _, key)| *key).fold(floor, u64::max);
            let mut held_items: Vec<_> = wrote.items.iter().filter(|(identity, at, _)| held_at(identity, *at)).cloned().collect();
            held_items.sort();
            kept_held.sort();
            met[0] += usize::from(wrote.items.len() < now.len());
            met[1] += usize::from(wrote.gone > 0);
            met[2] += usize::from(now.iter().zip(&identities).any(|(event, identity)| event.at < since && again.contains(identity)));
            met[3] += usize::from(!held_items.is_empty());
            assert_eq!((kept.written, kept.gone, kept.floor.max(floor)), (wrote.written, wrote.gone, settled_floor), "round {round}");
            assert_eq!(kept_held, held_items, "round {round}");
            assert_eq!(all(&kept_library), all(&held_library), "round {round}");
            drop(spill);
            let _ = super::super::spill::clear(&spill_root);
        }
        assert!(met.iter().all(|count| *count >= 3), "{met:?}");
        let _ = fastctx_platform::disk::remove_dir_all(&base);
    }
}
