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
/// Proofs kept before the lapsed ones are let go.
const PROOFS_KEPT: usize = 4096;
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
    /// The bodies a reply of frames sends apart.
    Bodies { machine: MachineName, key: [u8; 32], scope: Scope, at: Instant },
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

/// A proof of holding: the device's key and the object's hash.
type Proof = ([u8; 32], [u8; 32]);

/// The tickets handed out, and the proofs of holding that stand, each under
/// the key of the device that gave it.
#[derive(Default)]
pub(crate) struct Objects {
    tickets: Mutex<HashMap<u64, Ticket>>,
    proofs: Mutex<HashMap<Proof, Instant>>,
    pub(crate) apart: super::apart::Apart,
}

impl Objects {
    fn ticket(&self, ticket: Ticket) -> Result<u64, String> {
        let number = u64::from_le_bytes(fastctx_net::sign::random::<8>().map_err(|error| error.to_string())?);
        let mut tickets = self.tickets.lock().expect("tickets");
        tickets.retain(|_, kept| kept.at().elapsed() < TICKET_FOR);
        tickets.insert(number, ticket);
        Ok(number)
    }

    /// The objects among `hashes` the device whose key is `key` proved it
    /// holds lately.
    pub(crate) fn proven(&self, key: &[u8; 32], hashes: &[[u8; 32]]) -> Vec<[u8; 32]> {
        let proofs = self.proofs.lock().expect("proofs");
        hashes.iter().copied().filter(|hash| proofs.get(&(*key, *hash)).is_some_and(|until| Instant::now() < *until)).collect()
    }

    fn proved(&self, key: &[u8; 32], object: &ObjectRef) {
        let mut proofs = self.proofs.lock().expect("proofs");
        if proofs.len() >= PROOFS_KEPT {
            let now = Instant::now();
            proofs.retain(|_, until| now < *until);
        }
        proofs.insert((*key, object.hash), Instant::now() + proof_for(object.size));
    }
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
pub(crate) fn send_apart(assets: &Assets, machine: &MachineName, key: &PublicKey, scope: Scope, bodies: Vec<Vec<u8>>) -> Result<u64, String> {
    let ticket = assets.objects.ticket(Ticket::Bodies { machine: machine.clone(), key: *key.as_bytes(), scope, at: Instant::now() })?;
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
fn allowed(hub: &Hub, me: &MachineName, key: &PublicKey, scope: &Scope, object: Option<&ObjectRef>) -> Result<(), String> {
    if let Some(why) = hub.assets.sealed() {
        return Err(format!("every asset is sealed: {why}"));
    }
    let store = hub.store();
    let directory = store.indexed();
    if directory.member(me).is_none_or(|member| member.key != *key) {
        return Err(format!("{me} is no longer this machine"));
    }
    let fresh = scopes::scope(&hub.assets, &scope.descriptor.scope)?.ok_or("the scope is gone")?;
    if !scopes::readable(&directory, me, &fresh) || !scopes::sharers_read(&directory, me, &fresh) {
        return Err(format!("{me} reads the scope no more"));
    }
    if let Some(object) = object {
        let library = scopes::library(&hub.assets, &fresh)?;
        reachable(&directory, me, &fresh, &library, object)?;
    }
    Ok(())
}

/// `allowed`, off the async threads.
async fn still(hub: &Arc<Hub>, me: &MachineName, key: &PublicKey, scope: &Scope, object: Option<&ObjectRef>) -> Result<(), String> {
    let (hub, me, key, scope, object) = (hub.clone(), me.clone(), *key, scope.clone(), object.copied());
    fastctx_platform::task::spawn_blocking(move || allowed(&hub, &me, &key, &scope, object.as_ref()))
        .await
        .map_err(|error| error.to_string())?
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
    if let Err(why) = still(&hub, &me, &key, &scope, object.as_ref()).await {
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
        Ticket::Bodies { .. } => super::apart::send(&hub, socket, key.as_bytes(), number).await,
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

/// `length` bytes the member sends, in as many messages as it likes.
async fn bytes<S>(stream: &mut S, pulse: &Pulse, length: u64) -> Result<Vec<u8>, String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut bytes = Vec::with_capacity(length as usize);
    while (bytes.len() as u64) < length {
        match heard(stream, pulse).await? {
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
    let head: ChunkHead = serde_json::from_str(&word(stream, pulse).await?).map_err(|error| error.to_string())?;
    if head.size > CHUNK_BYTES {
        return Err("a chunk is at most 1 MiB".into());
    }
    Ok((head.index, bytes(stream, pulse, head.size).await?))
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
    let digests = bytes(stream, pulse, count * 32).await?;
    Ok(digests.chunks_exact(32).map(|digest| digest.try_into().expect("32 bytes")).collect())
}

/// Runs `work` where blocking is allowed.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> fastctx_content::Result<T> + Send + 'static) -> Result<T, String> {
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
            for expected in 0..chunks(object.size) {
                if expected > 0 && expected.is_multiple_of(CHECK_EVERY) {
                    still(hub, me, key, &scope, None).await?;
                }
                let (index, bytes) = chunk(&mut stream, &pulse).await?;
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
        while !wanted.is_empty() {
            if taken > 0 && taken.is_multiple_of(CHECK_EVERY) {
                still(hub, me, key, &scope, None).await?;
            }
            let (index, bytes) = chunk(&mut stream, &pulse).await?;
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
        hub.assets.objects.proved(key.as_bytes(), &object);
    }
    let word = match &kept {
        Ok(()) => DONE.to_string(),
        Err(why) => serde_json::to_string(&format!("the hub did not take it: {why}")).expect("text serializes"),
    };
    if let Err(why) = writer.finish(word).await {
        hub.log.line(format_args!("sending an object's receipt: {why}"));
    }
    kept
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
