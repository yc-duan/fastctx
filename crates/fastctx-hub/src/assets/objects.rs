//! Objects between a member and the hub (design §4.3, §6.5, §7.3): a
//! request on the control connection names the object and gets a ticket,
//! bound to the asking machine's key; a data connection under that ticket
//! carries its chunks, each a `ChunkHead` then its bytes. A large object's
//! chunk digests travel on the data connection too, ahead of its chunks: a
//! put sends them and hears which chunks the hub wants; a get hears them
//! first. A chunk is written to the transfer's part file as it comes; the
//! object counts once all of it hashes to what it was put as, leased until
//! a record refers to it. Sent though the hub holds it, every byte of it
//! proves the device holds it: the proof, under the device's key, stands
//! ten minutes for frames that refer to it (§6.5).
//!
//! Whether the machine may still read the object is judged again as the
//! data connection opens and every few chunks after, by the directory as it
//! stands then; a sealed hub gives and takes nothing.

use super::Assets;
use super::scopes::{self, Scope};
use crate::updates::receipt_writer::Writer;
use crate::updates::transfer::{heard, put as write};
use crate::{Hub, Socket, refuse, send};
use fastctx_content::ObjectRef;
use fastctx_content::v2::{CHUNK_BYTES, LEASE, Library, ROW_BYTES};
use fastctx_net::directory_index::Indexed;
use fastctx_net::pulse::Pulse;
use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{Down, HEARTBEAT, Refusal, SILENCE};
use fastctx_types::MachineName;
use fastctx_types::assets::{ChunkHead, ManifestHead, ObjectRef as WireObject, Reply, Wanted};
use futures_util::StreamExt as _;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// How long a ticket waits for its data connection.
const TICKET_FOR: Duration = Duration::from_secs(120);
/// How long a proof of holding stands (design §6.5: ten minutes, and the
/// time reading the object takes at 50 MiB/s).
fn proof_for(size: u64) -> Duration {
    Duration::from_secs(600) + Duration::from_secs(size / (50 << 20))
}
/// What the member says once it has every byte.
pub(super) const DONE: &str = "\"done\"";
/// How long the member has to say it holds an object once all of it went.
pub(super) const RECEIPT_WITHIN: Duration = Duration::from_secs(120);
/// Chunks between two judgements of whether the machine may go on.
const CHECK_EVERY: u64 = 16;
/// Proofs one device has standing at most, four times what one frame may
/// refer to (AD01, AE01), and every device's together.
const DEVICE_PROOFS: usize = 4 * fastctx_content::FRAME_OBJECTS;
const PROOFS_KEPT: usize = 1 << 18;
/// How often a device putting an object keeps what it proved and staged.
const KEEP_EVERY: Duration = Duration::from_secs(30);

/// One device's proofs: each object's, until when it stands.
type Proven = HashMap<[u8; 32], Instant>;
/// Ranges of chunks one put asks for at most: the rest are asked for at the
/// next, so a transfer left full of holes never asks in one message more
/// than a connection carries.
const RANGES_AT_ONCE: usize = 16_384;

/// What a ticket lets its data connection do.
#[derive(Clone)]
enum Ticket {
    /// `missing` names the chunks wanted; none for a large object put, whose
    /// digests come on the data connection first.
    Put { machine: MachineName, key: [u8; 32], scope: Scope, object: ObjectRef, missing: Option<Vec<u64>>, prove: bool, at: Instant },
    Get { machine: MachineName, key: [u8; 32], scope: Scope, object: ObjectRef, from: u64, at: Instant },
    /// A frame body staged apart (`apart`).
    Stage { machine: MachineName, key: [u8; 32], scope: Scope, size: u64, at: Instant },
    /// The bodies of `stream`'s frames a reply sends apart.
    Bodies { machine: MachineName, key: [u8; 32], scope: Scope, stream: String, at: Instant },
}

impl Ticket {
    fn holder(&self) -> (&MachineName, &[u8; 32]) {
        match self {
            Self::Put { machine, key, .. } | Self::Get { machine, key, .. } | Self::Stage { machine, key, .. } | Self::Bodies { machine, key, .. } => {
                (machine, key)
            }
        }
    }

    fn at(&self) -> Instant {
        match self {
            Self::Put { at, .. } | Self::Get { at, .. } | Self::Stage { at, .. } | Self::Bodies { at, .. } => *at,
        }
    }

    fn scope(&self) -> &Scope {
        match self {
            Self::Put { scope, .. } | Self::Get { scope, .. } | Self::Stage { scope, .. } | Self::Bodies { scope, .. } => scope,
        }
    }
}


/// The tickets handed out, and the proofs of holding that stand, each under
/// the key of the device that gave it.
#[derive(Default)]
pub(crate) struct Objects {
    tickets: Mutex<HashMap<u64, Ticket>>,
    /// By the proving device's key, each object's proof until when it stands.
    proofs: Mutex<HashMap<[u8; 32], Proven>>,
    pub(crate) apart: super::apart::Apart,
    /// What is asked and proved by challenge (design §28.45).
    pub(crate) proving: super::proving::Proving,
}

impl Objects {
    fn ticket(&self, ticket: Ticket) -> Result<u64, String> {
        let number = u64::from_le_bytes(fastctx_net::sign::random::<8>().map_err(|error| error.to_string())?);
        let mut tickets = self.tickets.lock().expect("tickets");
        tickets.retain(|_, kept| kept.at().elapsed() < TICKET_FOR);
        tickets.insert(number, ticket);
        Ok(number)
    }

    /// The proofs of `hashes` the device whose key is `key` gave go: a frame
    /// of them is applied, and they are read through it now (AF04).
    pub(crate) fn forget_proofs(&self, key: &[u8; 32], hashes: &[[u8; 32]]) {
        let mut proofs = self.proofs.lock().expect("proofs");
        if let Some(proven) = proofs.get_mut(key) {
            for hash in hashes {
                proven.remove(hash);
            }
            if proven.is_empty() {
                proofs.remove(key);
            }
        }
    }

    /// Whether the device whose key is `key` proved lately it holds the
    /// object of `hash`.
    pub(crate) fn proven(&self, key: &[u8; 32], hash: &[u8; 32]) -> bool {
        let proofs = self.proofs.lock().expect("proofs");
        proofs.get(key).and_then(|proven| proven.get(hash)).is_some_and(|until| Instant::now() < *until)
    }

    /// While the device whose key is `key` puts an object, what it proved
    /// lately and the bodies it staged stay (AA05): a frame that waits for
    /// several objects is not held up by the first one's proof, or its
    /// staged body, ending while the last one comes.
    pub(crate) fn keep_alive(&self, key: &[u8; 32]) {
        let now = Instant::now();
        let until = now + proof_for(0);
        if let Some(proven) = self.proofs.lock().expect("proofs").get_mut(key) {
            for kept in proven.values_mut().filter(|kept| **kept > now && **kept < until) {
                *kept = until;
            }
        }
        self.apart.keep_staged(key);
    }

    /// Keeps that the device whose key is `key` proved it holds `object`:
    /// sent every byte of it, or answered a challenge for it (§28.45).
    pub(crate) fn proved(&self, key: &[u8; 32], object: &ObjectRef) {
        let now = Instant::now();
        let mut proofs = self.proofs.lock().expect("proofs");
        if proofs.values().map(HashMap::len).sum::<usize>() >= PROOFS_KEPT {
            thin(&mut proofs, now);
        }
        let proven = proofs.entry(*key).or_default();
        if proven.len() >= DEVICE_PROOFS {
            proven.retain(|_, until| now < *until);
            // Past the bound with every proof still good, the quarter that
            // ends soonest goes, a quarter by count however many end alike
            // (AD08): a device asked again proves again.
            if proven.len() >= DEVICE_PROOFS {
                let mut ends: Vec<([u8; 32], Instant)> = proven.iter().map(|(hash, until)| (*hash, *until)).collect();
                ends.sort_unstable_by_key(|(_, until)| *until);
                for (hash, _) in ends.iter().take(ends.len() / 4) {
                    proven.remove(hash);
                }
            }
        }
        proven.insert(object.hash, now + proof_for(object.size));
        drop(proofs);
        // The frame it is proved for waits on (AD09).
        self.apart.keep_staged(key);
    }
}

/// Every device's proofs at their bound together: those that ended go; if
/// that is not enough, the devices with the most are cut down to one
/// number — never below what one frame refers to, so each kept can still
/// prove a whole frame (AF04) — each losing those of its own that end
/// soonest; and if even that is too many, the devices with the fewest lose
/// all theirs, those nearest a whole frame kept. Until three quarters of the
/// bound are left (AE05): the next walk is a quarter of the bound away.
fn thin(proofs: &mut HashMap<[u8; 32], Proven>, now: Instant) {
    for proven in proofs.values_mut() {
        proven.retain(|_, until| now < *until);
    }
    proofs.retain(|_, proven| !proven.is_empty());
    let target = PROOFS_KEPT / 4 * 3;
    let floor = fastctx_content::FRAME_OBJECTS;
    let kept_under = |most: usize| proofs.values().map(|proven| proven.len().min(most)).sum::<usize>();
    if kept_under(usize::MAX) <= target {
        return;
    }
    // Floored at a frame's worth, the devices with the fewest go whole.
    if kept_under(floor) > target {
        let mut by_count: Vec<([u8; 32], usize)> = proofs.iter().map(|(key, proven)| (*key, proven.len())).collect();
        by_count.sort_unstable_by_key(|(_, count)| *count);
        let mut total = kept_under(floor);
        for (key, count) in by_count {
            if total <= target {
                break;
            }
            total -= count.min(floor);
            proofs.remove(&key);
        }
    }
    let kept_under = |most: usize| proofs.values().map(|proven| proven.len().min(most)).sum::<usize>();
    // The most each device keeps: the largest number, a frame's worth at
    // least, that leaves the target.
    let (mut low, mut high) = (floor, proofs.values().map(HashMap::len).max().unwrap_or(0).max(floor));
    while low < high {
        let middle = (low + high).div_ceil(2);
        if kept_under(middle) <= target { low = middle } else { high = middle - 1 }
    }
    for proven in proofs.values_mut() {
        if proven.len() > low {
            let mut ends: Vec<([u8; 32], Instant)> = proven.iter().map(|(hash, until)| (*hash, *until)).collect();
            ends.sort_unstable_by_key(|(_, until)| *until);
            for (hash, _) in ends.iter().take(ends.len() - low) {
                proven.remove(hash);
            }
        }
    }
    proofs.retain(|_, proven| !proven.is_empty());
}

fn local(object: &WireObject) -> Result<ObjectRef, String> {
    Ok(ObjectRef { hash: object.hash.array().ok_or("an object's hash is 32 bytes")?, size: object.size })
}

fn chunks(size: u64) -> u64 {
    size.div_ceil(CHUNK_BYTES).max(1)
}

fn large(object: &ObjectRef) -> bool {
    object.size as usize > ROW_BYTES
}

/// `indices`, in order, as ranges [start, end).
fn ranges(indices: &[u64]) -> Vec<(u64, u64)> {
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    for &index in indices {
        match ranges.last_mut() {
            Some((_, end)) if *end == index => *end += 1,
            _ => ranges.push((index, index + 1)),
        }
    }
    ranges
}

/// A put asked on the control connection (`Request::Put`): which chunks
/// are wanted, under what ticket; a large object's are said once its
/// digests came on the data connection.
pub(crate) fn put(assets: &Assets, machine: &MachineName, key: &PublicKey, scope: Scope, object: &WireObject, prove: bool) -> Result<Reply, String> {
    let object = local(object)?;
    let library = scopes::library(assets, &scope)?;
    let held = library.size(&object.hash).map_err(|error| error.to_string())?;
    // A proof is of an object the hub holds at that length: asked of one it
    // does not, nothing is sized by what the request says (AQ10).
    if prove && held != Some(object.size) {
        return Err("a proof is of an object the hub holds at that length".into());
    }
    let missing = if prove {
        Some((0..chunks(object.size)).collect())
    } else if held == Some(object.size) {
        return Ok(Reply::Put { ticket: 0, missing: Vec::new() });
    } else if large(&object) {
        // Every chunk of it in already, a stop before it was placed: it is
        // placed now, not asked for again.
        if let Some(transfer) = library.under_way(object).map_err(|error| error.to_string())?
            && transfer.missing().is_empty()
        {
            transfer.finish(&format!("upload:{machine}"), LEASE).map_err(|error| error.to_string())?;
            return Ok(Reply::Put { ticket: 0, missing: Vec::new() });
        }
        None
    } else {
        Some(vec![0])
    };
    // A proof is every chunk, in order: none is listed.
    let said = if prove { Vec::new() } else { missing.clone().unwrap_or_default() };
    let ticket = assets.objects.ticket(Ticket::Put {
        machine: machine.clone(),
        key: *key.as_bytes(),
        scope,
        object,
        missing,
        prove,
        at: Instant::now(),
    })?;
    Ok(Reply::Put { ticket, missing: said })
}

/// Room for a frame body of `size` bytes, too large for the control
/// connection (`Request::Stage`): the ticket a data connection sends it
/// under.
pub(crate) fn stage(assets: &Assets, machine: &MachineName, key: &PublicKey, scope: Scope, size: u64) -> Result<Reply, String> {
    let ticket = assets.objects.ticket(Ticket::Stage { machine: machine.clone(), key: *key.as_bytes(), scope, size, at: Instant::now() })?;
    if let Err(why) = super::apart::stage(&assets.objects.apart, ticket, key.as_bytes(), size) {
        assets.objects.tickets.lock().expect("tickets").remove(&ticket);
        return Err(why);
    }
    Ok(Reply::Staged { ticket })
}

/// A ticket for `bodies`, in order, of a reply of frames to `machine`.
pub(crate) fn send_apart(
    assets: &Assets,
    machine: &MachineName,
    key: &PublicKey,
    scope: Scope,
    stream: &str,
    bodies: Vec<Vec<u8>>,
) -> Result<u64, String> {
    let ticket = assets.objects.ticket(Ticket::Bodies { machine: machine.clone(), key: *key.as_bytes(), scope, stream: stream.to_string(), at: Instant::now() })?;
    if let Err(why) = super::apart::send_later(&assets.objects.apart, ticket, key.as_bytes(), bodies) {
        assets.objects.tickets.lock().expect("tickets").remove(&ticket);
        return Err(why);
    }
    Ok(ticket)
}

/// Whether `machine` reaches `object` in `scope`: through a stream of it
/// the machine may hold (design §2.3), not merely through the scope.
fn reachable(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope, library: &Library, object: &ObjectRef) -> Result<(), String> {
    let streams = library.streams_reading(&scope.descriptor.scope, &object.hash).map_err(|error| error.to_string())?;
    if streams.iter().any(|stream| scopes::holds(directory, machine, scope, stream)) {
        Ok(())
    } else {
        Err("the object is read through no stream of the scope this machine holds".into())
    }
}

/// A get asked on the control connection (`Request::Get`): the ticket the
/// object's chunks come under.
pub(crate) fn get(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    key: &PublicKey,
    scope: Scope,
    object: &WireObject,
    from: u64,
) -> Result<Reply, String> {
    let object = local(object)?;
    let library = scopes::library(assets, &scope)?;
    reachable(directory, machine, &scope, &library, &object)?;
    if library.size(&object.hash).map_err(|error| error.to_string())? != Some(object.size) {
        return Err("the hub does not hold the object".into());
    }
    let ticket =
        assets.objects.ticket(Ticket::Get { machine: machine.clone(), key: *key.as_bytes(), scope, object, from, at: Instant::now() })?;
    Ok(Reply::Get { ticket, manifest: Vec::new() })
}

/// Whether `me`, whose key is `key`, may go on with an object of `scope` —
/// a get's `object` — by the directory as it stands now. Call on a thread
/// that may block.
fn allowed(hub: &Hub, me: &MachineName, key: &PublicKey, scope: &Scope, object: Option<&ObjectRef>, stream: Option<&str>) -> Result<(), String> {
    if let Some(why) = hub.assets.sealed() {
        return Err(format!("every asset is sealed: {why}"));
    }
    let store = hub.store();
    let directory = store.indexed();
    if directory.member(me).is_none_or(|member| member.key != *key) {
        return Err(format!("{me} is no longer this machine"));
    }
    let fresh = scopes::scope(&hub.assets, &scope.descriptor.scope)?.ok_or("the scope is gone")?;
    if !scopes::readable(&directory, me, &fresh) {
        return Err(format!("{me} reads the scope no more"));
    }
    if let Some(object) = object {
        let library = scopes::library(&hub.assets, &fresh)?;
        reachable(&directory, me, &fresh, &library, object)?;
    }
    if let Some(stream) = stream
        && !scopes::holds(&directory, me, &fresh, stream)
    {
        return Err(format!("{me} holds {stream} of the scope no more"));
    }
    Ok(())
}

/// `allowed`, off the async threads.
async fn still(hub: &Arc<Hub>, me: &MachineName, key: &PublicKey, scope: &Scope, object: Option<&ObjectRef>) -> Result<(), String> {
    still_holding(hub, me, key, scope, object, None).await
}

/// `still`, of `stream` too when one is named.
pub(super) async fn still_holding(
    hub: &Arc<Hub>,
    me: &MachineName,
    key: &PublicKey,
    scope: &Scope,
    object: Option<&ObjectRef>,
    stream: Option<&str>,
) -> Result<(), String> {
    let (hub, me, key, scope, object, stream) = (hub.clone(), me.clone(), *key, scope.clone(), object.copied(), stream.map(str::to_string));
    fastctx_platform::task::spawn_blocking(move || allowed(&hub, &me, &key, &scope, object.as_ref(), stream.as_deref()))
        .await
        .map_err(|error| error.to_string())?
}

/// The scope `key` an asset line asks objects of or puts one into, judged
/// as any ask of it is: what the hub answers instead where the assets are
/// sealed or `origin` reads the scope no more. Call on a thread that may
/// block.
pub(super) fn line_scope(hub: &Hub, origin: &crate::origin::Origin, key: &str) -> Result<Result<Scope, Reply>, String> {
    let assets = &hub.assets;
    if let Some(why) = assets.sealed() {
        return Ok(Err(assets.sealed_reply(why)));
    }
    let store = hub.store();
    origin.check(&store.book.directory)?;
    let directory = store.indexed();
    if assets.was_deleted(key) {
        assets.gone(key, &origin.name);
        return Ok(Err(Reply::Revoked { scope: key.to_string() }));
    }
    if let Some(scope) = scopes::scope(assets, key)?
        && !scopes::readable(&directory, &origin.name, &scope)
    {
        assets.gone(key, &origin.name);
        return Ok(Err(Reply::Revoked { scope: key.to_string() }));
    }
    super::serve::gated(assets, &directory, &origin.name, key).map(Ok)
}

/// An object an asset line gives `origin`, judged as a get's is by the
/// directory as it stands now: the library it is read from. Call on a
/// thread that may block.
pub(super) fn line_object(hub: &Hub, origin: &crate::origin::Origin, scope: &Scope, object: &WireObject) -> Result<(Library, ObjectRef), String> {
    let object = local(object)?;
    if object.size > fastctx_types::asset_lines::OBJECT_BYTES {
        return Err(format!("an object over {} bytes travels under a ticket", fastctx_types::asset_lines::OBJECT_BYTES));
    }
    allowed(hub, &origin.name, &origin.key, scope, Some(&object), None)?;
    let library = scopes::library(&hub.assets, scope)?;
    if library.size(&object.hash).map_err(|error| error.to_string())? != Some(object.size) {
        return Err("the hub does not hold the object".into());
    }
    Ok((library, object))
}

/// A put asked on an asset line: the library the object's bytes are kept
/// in when the hub wants them, none when it holds the object already; what
/// the hub answers instead of a scope revoked or assets sealed. Call on a
/// thread that may block.
#[allow(clippy::type_complexity)]
pub(super) fn line_put(
    hub: &Hub,
    origin: &crate::origin::Origin,
    scope: &str,
    object: &WireObject,
    prove: bool,
) -> Result<Result<Option<(Library, ObjectRef)>, Reply>, String> {
    let object = local(object)?;
    if object.size > fastctx_types::asset_lines::OBJECT_BYTES {
        return Err(format!("an object over {} bytes travels under a ticket", fastctx_types::asset_lines::OBJECT_BYTES));
    }
    let kept = match line_scope(hub, origin, scope)? {
        Ok(kept) => kept,
        Err(reply) => return Ok(Err(reply)),
    };
    let library = scopes::library(&hub.assets, &kept)?;
    let held = library.size(&object.hash).map_err(|error| error.to_string())?;
    // A proof is of an object the hub holds at that length (AQ10).
    if prove && held != Some(object.size) {
        return Err("a proof is of an object the hub holds at that length".into());
    }
    if !prove && held == Some(object.size) {
        return Ok(Ok(None));
    }
    Ok(Ok(Some((library, object))))
}

/// The hub's end of an object's data connection under `ticket`.
pub(crate) async fn serve(hub: Arc<Hub>, mut socket: Socket, binding: [u8; 32], key: PublicKey, proof: Signature, ticket: u64) {
    let me = match hub.member(&key, &proof, &binding) {
        Ok(me) => me,
        Err(reason) => {
            refuse(&mut socket, reason).await;
            return;
        }
    };
    let number = ticket;
    let taken = hub.assets.objects.tickets.lock().expect("tickets").remove(&ticket);
    let ticket = taken.filter(|kept| kept.holder() == (&me, key.as_bytes()) && kept.at().elapsed() < TICKET_FOR);
    let Some(ticket) = ticket else {
        refuse(&mut socket, Refusal::Broken { reason: format!("the hub gave {me} no such object ticket") }).await;
        return;
    };
    let scope = ticket.scope().clone();
    let object = match &ticket {
        Ticket::Get { object, .. } => Some(*object),
        _ => None,
    };
    let stream = match &ticket {
        Ticket::Bodies { stream, .. } => Some(stream.clone()),
        _ => None,
    };
    if let Err(why) = still_holding(&hub, &me, &key, &scope, object.as_ref(), stream.as_deref()).await {
        refuse(&mut socket, Refusal::Broken { reason: why }).await;
        return;
    }
    if tokio::time::timeout(SILENCE, send(&mut socket, &Down::Paired)).await.map_or(true, |sent| sent.is_err()) {
        return;
    }
    let outcome = match ticket {
        Ticket::Put { scope, object, missing, prove, .. } => take(&hub, socket, &me, &key, Put { scope, object, missing, prove }).await,
        Ticket::Get { scope, object, from, .. } => give(&hub, socket, &me, &key, scope, object, from).await,
        Ticket::Stage { size, .. } => super::apart::receive(&hub, socket, &me, key.as_bytes(), number, size).await,
        Ticket::Bodies { .. } => {
            let stream = stream.clone().unwrap_or_default();
            super::apart::send(&hub, socket, &me, &key, &scope, &stream, number).await
        }
    };
    if let Err(why) = outcome {
        hub.log.line(format_args!("an object with {me}: {why}"));
    }
}

/// What a put's connection brings.
struct Put {
    scope: Scope,
    object: ObjectRef,
    missing: Option<Vec<u64>>,
    prove: bool,
}

/// The next text the member says, past its pings.
async fn word<S>(stream: &mut S, pulse: &Pulse) -> Result<String, String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match heard(stream, pulse).await? {
            Some(WsMessage::Text(text)) => return Ok(text.to_string()),
            Some(WsMessage::Ping(_) | WsMessage::Pong(_)) => {}
            Some(WsMessage::Close(_)) | None => return Err("the member left before it sent it all".into()),
            Some(_) => return Err("bytes came where words were due".into()),
        }
    }
}

/// Keeps what the device putting an object proved and staged, by time, as
/// its bytes come: however long one chunk takes, nothing it proved ends
/// while it is still sending (CC01).
pub(super) struct Keeper<'a> {
    objects: &'a Objects,
    key: [u8; 32],
    kept: Option<Instant>,
}

impl<'a> Keeper<'a> {
    pub(super) fn new(objects: &'a Objects, key: [u8; 32]) -> Self {
        Self { objects, key, kept: None }
    }

    pub(super) fn tick(&mut self) {
        if self.kept.is_none_or(|kept| kept.elapsed() >= KEEP_EVERY) {
            self.objects.keep_alive(&self.key);
            self.kept = Some(Instant::now());
        }
    }
}

/// `length` bytes the member sends, in as many messages as it likes.
async fn bytes<S>(stream: &mut S, pulse: &Pulse, length: u64, mut keeper: Option<&mut Keeper<'_>>) -> Result<Vec<u8>, String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut bytes = Vec::with_capacity(length as usize);
    while (bytes.len() as u64) < length {
        if let Some(keeper) = keeper.as_deref_mut() {
            keeper.tick();
        }
        match heard(stream, pulse).await? {
            // Refused before it is kept: no more is kept than was said (AC05).
            Some(WsMessage::Binary(piece)) if bytes.len() as u64 + piece.len() as u64 > length => {
                return Err("more bytes came than were said".into());
            }
            Some(WsMessage::Binary(piece)) => bytes.extend_from_slice(&piece),
            Some(WsMessage::Ping(_) | WsMessage::Pong(_)) => {}
            Some(WsMessage::Close(_)) | None => return Err("the member left before it sent it all".into()),
            Some(_) => return Err("bytes were cut short".into()),
        }
    }
    if bytes.len() as u64 != length {
        return Err("more bytes came than were said".into());
    }
    Ok(bytes)
}

/// The next chunk the member sends: its head, then its bytes.
pub(super) async fn chunk<S>(stream: &mut S, pulse: &Pulse) -> Result<(u64, Vec<u8>), String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    chunk_at_most(stream, pulse, CHUNK_BYTES).await
}

/// The next chunk, what its sender proved and staged kept as it comes.
async fn chunk_kept<S>(stream: &mut S, pulse: &Pulse, keeper: &mut Keeper<'_>) -> Result<(u64, Vec<u8>), String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    keeper.tick();
    let head: ChunkHead = serde_json::from_str(&word(stream, pulse).await?).map_err(|error| error.to_string())?;
    if head.size > CHUNK_BYTES {
        return Err(format!("a chunk here is at most {CHUNK_BYTES} bytes"));
    }
    Ok((head.index, bytes(stream, pulse, head.size, Some(keeper)).await?))
}

/// The next chunk the member sends, refused before its bytes come when it
/// says it is longer than `most`.
pub(super) async fn chunk_at_most<S>(stream: &mut S, pulse: &Pulse, most: u64) -> Result<(u64, Vec<u8>), String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let head: ChunkHead = serde_json::from_str(&word(stream, pulse).await?).map_err(|error| error.to_string())?;
    if head.size > CHUNK_BYTES.min(most) {
        return Err(format!("a chunk here is at most {} bytes", CHUNK_BYTES.min(most)));
    }
    Ok((head.index, bytes(stream, pulse, head.size, None).await?))
}

/// A large object's chunk digests, as the member sends them.
async fn manifest<S>(stream: &mut S, pulse: &Pulse, count: u64) -> Result<Vec<[u8; 32]>, String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let head: ManifestHead = serde_json::from_str(&word(stream, pulse).await?).map_err(|error| error.to_string())?;
    if head.count != count {
        return Err("a large object comes with one digest a chunk".into());
    }
    let digests = bytes(stream, pulse, count * 32, None).await?;
    Ok(digests.chunks_exact(32).map(|digest| digest.try_into().expect("32 bytes")).collect())
}

/// Runs `work` where blocking is allowed.
pub(super) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> fastctx_content::Result<T> + Send + 'static) -> Result<T, String> {
    fastctx_platform::task::spawn_blocking(work).await.map_err(|error| error.to_string())?.map_err(|error| error.to_string())
}

/// Takes the chunks of an object put, or proved, under its ticket, and
/// says whether the hub took it.
async fn take(hub: &Arc<Hub>, mut socket: Socket, me: &MachineName, key: &PublicKey, put: Put) -> Result<(), String> {
    let library = scopes::library(&hub.assets, &put.scope)?;
    let pulse = crate::pulse_of(&socket);
    let Put { scope, object, missing, prove } = put;
    // A large object's digests come first; the hub says which chunks it
    // wants, a transfer under way taken up where its chunks stopped.
    let (missing, transfer, partly) = match missing {
        Some(missing) => (missing, None, false),
        None => {
            let digests = manifest(&mut socket, &pulse, chunks(object.size)).await?;
            let (receiving, source) = (library.clone(), me.to_string());
            let transfer = blocking(move || receiving.receive(object, digests, Some(source))).await?;
            let missing = transfer.as_ref().map_or_else(Vec::new, fastctx_content::v2::Transfer::missing);
            let mut asked = ranges(&missing);
            let partly = asked.len() > RANGES_AT_ONCE;
            asked.truncate(RANGES_AT_ONCE);
            let missing: Vec<u64> = asked.iter().flat_map(|&(start, end)| start..end).collect();
            let wanted = serde_json::to_string(&Wanted { ranges: asked }).expect("ranges serialize");
            write(&mut socket, &pulse, WsMessage::Text(wanted.into())).await?;
            (missing, transfer, partly)
        }
    };
    let (sink, mut stream) = socket.split();
    let work = fastctx_net::data::ReceiptWork::default();
    let writer = Writer::new(sink, pulse.clone(), work.clone());
    let holder = format!("upload:{me}");
    // A put of every byte proves holding as well as a proof does.
    let whole = prove || missing.len() as u64 == chunks(object.size);
    let kept: Result<(), String> = async {
        if prove {
            // Every byte, in order, hashed as it comes.
            let mut hasher = Sha256::new();
            let mut length = 0u64;
            // Kept by time, however few chunks the object has (BB02) and
            // however slowly one comes (CC01).
            let mut keeper = Keeper::new(&hub.assets.objects, *key.as_bytes());
            for expected in 0..chunks(object.size) {
                if expected > 0 && expected.is_multiple_of(CHECK_EVERY) {
                    still(hub, me, key, &scope, None).await?;
                }
                let (index, bytes) = chunk_kept(&mut stream, &pulse, &mut keeper).await?;
                if index != expected {
                    return Err("a proof's chunks come in order".into());
                }
                let _hashing = work.begin();
                length += bytes.len() as u64;
                hasher.update(&bytes);
            }
            if length != object.size || <[u8; 32]>::from(hasher.finalize()) != object.hash {
                return Err("the bytes sent do not hash to the object".into());
            }
            return Ok(());
        }
        if !large(&object) {
            let (_, bytes) = chunk(&mut stream, &pulse).await?;
            let _putting = work.begin();
            let _keeping = keeping(me, &object);
            let lease = blocking(move || library.put(&mut bytes.as_slice(), &holder, LEASE)).await?;
            if lease.object != object {
                return Err("the bytes sent do not hash to the object".into());
            }
            return Ok(());
        }
        let mut transfer = transfer;
        // Present meanwhile: the chunks still come, and where they are all
        // of it they are hashed here, as no transfer checks them whole.
        let mut hasher = (transfer.is_none() && whole).then(Sha256::new);
        let mut wanted: BTreeSet<u64> = missing.into_iter().collect();
        let (mut next, mut taken) = (0, 0u64);
        let mut keeper = Keeper::new(&hub.assets.objects, *key.as_bytes());
        while !wanted.is_empty() {
            if taken > 0 && taken.is_multiple_of(CHECK_EVERY) {
                still(hub, me, key, &scope, None).await?;
            }
            let (index, bytes) = chunk_kept(&mut stream, &pulse, &mut keeper).await?;
            taken += 1;
            if !wanted.remove(&index) {
                return Err(format!("chunk {index} was not asked for"));
            }
            let _writing = work.begin();
            if let Some(hasher) = hasher.as_mut() {
                if index != next {
                    return Err("a whole object's chunks come in order".into());
                }
                next += 1;
                hasher.update(&bytes);
            }
            if let Some(mut writing) = transfer.take() {
                transfer = Some(blocking(move || writing.chunk(index, &bytes).map(|()| writing)).await?);
            }
        }
        let _settling = work.begin();
        let _keeping = keeping(me, &object);
        if let Some(hasher) = hasher
            && <[u8; 32]>::from(hasher.finalize()) != object.hash
        {
            return Err("the bytes sent do not hash to the object".into());
        }
        if let Some(mut transfer) = transfer {
            if partly {
                // What came is kept; the rest is asked for at the next put.
                blocking(move || transfer.settle()).await?;
            } else {
                blocking(move || transfer.finish(&holder, LEASE)).await?;
            }
        }
        Ok(())
    }
    .await;
    if kept.is_ok() && whole {
        let _proving = keeping(me, &object);
        hub.assets.objects.proved(key.as_bytes(), &object);
    }
    let word = match &kept {
        Ok(()) => DONE.to_string(),
        Err(why) => serde_json::to_string(&format!("the hub did not take it: {why}")).expect("text serializes"),
    };
    let _receipt = fastctx_platform::doing::waiting_outside(
        format!("{me}'s receipt for an object it put ({} bytes)", object.size),
        SILENCE * 2,
    );
    if let Err(why) = writer.finish(word).await {
        hub.log.line(format_args!("sending an object's receipt: {why}"));
    }
    kept
}

/// Names the hub keeping an object a device put, past the bytes' coming,
/// so a keep that does not end says where it stands; the device's receipt
/// waits on it for as long as it lasts.
fn keeping(me: &MachineName, object: &ObjectRef) -> fastctx_platform::doing::OutsideWait {
    fastctx_platform::doing::waiting_outside(format!("keeping an object {me} put ({} bytes)", object.size), SILENCE * 2)
}

/// Sends a large object's digests, then its chunks from the chunk `from`,
/// each its head then its bytes, and waits for the member's word that it
/// has them all.
async fn give(hub: &Arc<Hub>, socket: Socket, me: &MachineName, key: &PublicKey, scope: Scope, object: ObjectRef, from: u64) -> Result<(), String> {
    let library = scopes::library(&hub.assets, &scope)?;
    let pulse = crate::pulse_of(&socket);
    let (mut sink, mut stream) = socket.split();
    let (body_sent, sent) = tokio::sync::oneshot::channel();
    let receipt = async {
        let at = match sent.await {
            Ok(at) => at,
            Err(_) => return std::future::pending::<Result<(), String>>().await,
        };
        tokio::time::sleep_until(at + RECEIPT_WITHIN).await;
        Err(format!("the member did not say it had the object within {} s", RECEIPT_WITHIN.as_secs()))
    };
    let writer = async {
        let mut pace = fastctx_net::data::Pace::default();
        if large(&object) {
            let reading = library.clone();
            let digests = blocking(move || reading.chunks(&object.hash)).await?;
            let head = serde_json::to_string(&ManifestHead { count: digests.len() as u64 }).expect("a head serializes");
            write(&mut sink, &pulse, WsMessage::Text(head.into())).await?;
            let flat: Vec<u8> = digests.iter().flatten().copied().collect();
            for piece in flat.chunks(pace.chunk()) {
                let began = std::time::Instant::now();
                write(&mut sink, &pulse, WsMessage::Binary(piece.to_vec().into())).await?;
                pace.wrote(piece.len(), began.elapsed());
            }
        }
        for index in from..chunks(object.size) {
            if index > from && (index - from).is_multiple_of(CHECK_EVERY) {
                still(hub, me, key, &scope, Some(&object)).await?;
            }
            let offset = index * CHUNK_BYTES;
            let length = (object.size - offset).min(CHUNK_BYTES) as u32;
            let reading = library.clone();
            let bytes = blocking(move || reading.read(&object.hash, offset, length)).await?;
            let head = serde_json::to_string(&ChunkHead { index, size: bytes.len() as u64 }).expect("a head serializes");
            write(&mut sink, &pulse, WsMessage::Text(head.into())).await?;
            for piece in bytes.chunks(pace.chunk()) {
                let began = std::time::Instant::now();
                write(&mut sink, &pulse, WsMessage::Binary(piece.to_vec().into())).await?;
                pace.wrote(piece.len(), began.elapsed());
            }
        }
        let _ = body_sent.send(tokio::time::Instant::now());
        loop {
            tokio::time::sleep(HEARTBEAT).await;
            write(&mut sink, &pulse, fastctx_net::pulse::ping(&pulse)).await?;
        }
    };
    let reader = async {
        loop {
            match heard(&mut stream, &pulse).await? {
                Some(WsMessage::Text(text)) if text.as_str() == DONE => return Ok(()),
                Some(WsMessage::Close(_)) | None => return Err("the member left before it had it all".to_string()),
                Some(WsMessage::Ping(said)) => {
                    if let Some(took) = fastctx_net::pulse::said(&said) {
                        pulse.told(took);
                    }
                }
                Some(_) => {}
            }
        }
    };
    tokio::select! {
        ended = reader => ended,
        ended = writer => ended,
        ended = receipt => ended,
    }
}
