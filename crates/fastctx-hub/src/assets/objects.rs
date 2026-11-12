//! Objects between a member and the hub (design §4.3, §6.5, §7.3): a
//! request on the control connection names the object and gets a ticket; a
//! data connection under that ticket carries its chunks, each a `ChunkHead`
//! then its bytes. A chunk is written to the transfer's part file as it
//! comes; the object counts once all of it hashes to what it was put as,
//! leased until a record refers to it. Sent though the hub holds it, every
//! byte of it proves the member holds it: the proof stands ten minutes for
//! frames that refer to it (§6.5). The hub ends these connections itself.

use super::scopes::{self, Scope};
use super::Assets;
use crate::updates::receipt_writer::Writer;
use crate::updates::transfer::{heard, put as write};
use crate::{Hub, Socket, refuse, send};
use fastctx_content::ObjectRef;
use fastctx_content::v2::{CHUNK_BYTES, LEASE, ROW_BYTES};
use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::pulse::Pulse;
use fastctx_net::wire::{Down, HEARTBEAT, Refusal, SILENCE};
use fastctx_types::MachineName;
use fastctx_types::assets::{Bytes, ChunkHead, ObjectRef as WireObject, Reply};
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
const DONE: &str = "\"done\"";
/// How long the member has to say it holds an object once all of it went.
const RECEIPT_WITHIN: Duration = Duration::from_secs(120);

/// What a ticket lets its data connection do.
#[derive(Clone)]
enum Ticket {
    Put { machine: MachineName, scope: Scope, object: ObjectRef, manifest: Vec<[u8; 32]>, missing: Vec<u64>, prove: bool, at: Instant },
    Get { machine: MachineName, scope: Scope, object: ObjectRef, from: u64, at: Instant },
}

/// The tickets handed out, and the proofs of holding that stand.
#[derive(Default)]
pub(crate) struct Objects {
    tickets: Mutex<HashMap<u64, Ticket>>,
    proofs: Mutex<HashMap<(MachineName, [u8; 32]), Instant>>,
}

impl Objects {
    fn ticket(&self, ticket: Ticket) -> Result<u64, String> {
        let number = u64::from_le_bytes(fastctx_net::sign::random::<8>().map_err(|error| error.to_string())?);
        let mut tickets = self.tickets.lock().expect("tickets");
        tickets.retain(|_, kept| match kept {
            Ticket::Put { at, .. } | Ticket::Get { at, .. } => at.elapsed() < TICKET_FOR,
        });
        tickets.insert(number, ticket);
        Ok(number)
    }

    /// The objects among `hashes` `machine` proved it holds lately.
    pub(crate) fn proven(&self, machine: &MachineName, hashes: &[[u8; 32]]) -> Vec<[u8; 32]> {
        let proofs = self.proofs.lock().expect("proofs");
        hashes.iter().copied().filter(|hash| proofs.get(&(machine.clone(), *hash)).is_some_and(|until| Instant::now() < *until)).collect()
    }
}

fn local(object: &WireObject) -> Result<ObjectRef, String> {
    Ok(ObjectRef { hash: object.hash.array().ok_or("an object's hash is 32 bytes")?, size: object.size })
}

fn chunks(size: u64) -> u64 {
    size.div_ceil(CHUNK_BYTES).max(1)
}

/// A put asked on the control connection (`Request::Put`): which chunks
/// are wanted, under what ticket.
pub(crate) fn put(assets: &Assets, machine: &MachineName, scope: Scope, object: &WireObject, manifest: Vec<Bytes>, prove: bool) -> Result<Reply, String> {
    let object = local(object)?;
    let manifest: Vec<[u8; 32]> = manifest.iter().map(|digest| digest.array().ok_or("a chunk digest is 32 bytes")).collect::<Result<_, _>>()?;
    let large = object.size as usize > ROW_BYTES;
    if large && manifest.len() as u64 != chunks(object.size) {
        return Err("a large object comes with one digest a chunk".into());
    }
    let library = scopes::library(assets, &scope)?;
    let held = library.size(&object.hash).map_err(|error| error.to_string())?;
    let missing: Vec<u64> = if prove {
        (0..chunks(object.size)).collect()
    } else if held == Some(object.size) {
        Vec::new()
    } else if large {
        let transfer = library.receive(object, manifest.clone(), Some(machine.to_string())).map_err(|error| error.to_string())?;
        transfer.map_or_else(Vec::new, |transfer| transfer.missing())
    } else {
        vec![0]
    };
    if missing.is_empty() {
        return Ok(Reply::Put { ticket: 0, missing });
    }
    let ticket = assets.objects.ticket(Ticket::Put { machine: machine.clone(), scope, object, manifest, missing: missing.clone(), prove, at: Instant::now() })?;
    Ok(Reply::Put { ticket, missing })
}

/// A get asked on the control connection (`Request::Get`): the object's
/// chunks' digests, and the ticket they come under.
pub(crate) fn get(assets: &Assets, machine: &MachineName, scope: Scope, object: &WireObject, from: u64) -> Result<Reply, String> {
    let object = local(object)?;
    let library = scopes::library(assets, &scope)?;
    let key = scope.descriptor.scope.clone();
    let readable = library.readable(&key, &object.hash).map_err(|error| error.to_string())?
        || library.readable_as_evidence(&key, &object.hash).map_err(|error| error.to_string())?;
    if !readable {
        return Err("the object is not read in that scope".into());
    }
    if library.size(&object.hash).map_err(|error| error.to_string())? != Some(object.size) {
        return Err("the hub does not hold the object".into());
    }
    let manifest = if object.size as usize > ROW_BYTES {
        library.chunks(&object.hash).map_err(|error| error.to_string())?.into_iter().map(Bytes::from).collect()
    } else {
        Vec::new()
    };
    let ticket = assets.objects.ticket(Ticket::Get { machine: machine.clone(), scope, object, from, at: Instant::now() })?;
    Ok(Reply::Get { ticket, manifest })
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
    let taken = hub.assets.objects.tickets.lock().expect("tickets").remove(&ticket);
    let ticket = match taken {
        Some(Ticket::Put { machine, .. } | Ticket::Get { machine, .. }) if machine != me => None,
        Some(Ticket::Put { at, .. } | Ticket::Get { at, .. }) if at.elapsed() >= TICKET_FOR => None,
        other => other,
    };
    let Some(ticket) = ticket else {
        refuse(&mut socket, Refusal::Broken { reason: format!("the hub gave {me} no such object ticket") }).await;
        return;
    };
    if tokio::time::timeout(SILENCE, send(&mut socket, &Down::Paired)).await.map_or(true, |sent| sent.is_err()) {
        return;
    }
    let outcome = match ticket {
        Ticket::Put { scope, object, manifest, missing, prove, .. } => {
            take(&hub, socket, &me, Put { scope, object, manifest, missing, prove }).await
        }
        Ticket::Get { scope, object, from, .. } => give(&hub, socket, scope, object, from).await,
    };
    if let Err(why) = outcome {
        hub.log.line(format_args!("an object with {me}: {why}"));
    }
}

/// What a put's connection brings.
struct Put {
    scope: Scope,
    object: ObjectRef,
    manifest: Vec<[u8; 32]>,
    missing: Vec<u64>,
    prove: bool,
}

/// The next chunk the member sends: its head, then its bytes.
async fn chunk<S>(stream: &mut S, pulse: &Pulse) -> Result<(u64, Vec<u8>), String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let head: ChunkHead = loop {
        match heard(stream, pulse).await? {
            Some(WsMessage::Text(text)) => break serde_json::from_str(&text).map_err(|error| error.to_string())?,
            Some(WsMessage::Ping(_) | WsMessage::Pong(_)) => {}
            Some(WsMessage::Close(_)) | None => return Err("the member left before it sent it all".into()),
            Some(_) => return Err("bytes came before their chunk's head".into()),
        }
    };
    if head.size > CHUNK_BYTES {
        return Err("a chunk is at most 1 MiB".into());
    }
    let mut bytes = Vec::with_capacity(head.size as usize);
    while (bytes.len() as u64) < head.size {
        match heard(stream, pulse).await? {
            Some(WsMessage::Binary(piece)) => bytes.extend_from_slice(&piece),
            Some(WsMessage::Ping(_) | WsMessage::Pong(_)) => {}
            Some(WsMessage::Close(_)) | None => return Err("the member left before it sent it all".into()),
            Some(_) => return Err("a chunk's bytes were cut short".into()),
        }
    }
    if bytes.len() as u64 != head.size {
        return Err("a chunk is longer than its head said".into());
    }
    Ok((head.index, bytes))
}

/// Runs `work` where blocking is allowed.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> fastctx_content::Result<T> + Send + 'static) -> Result<T, String> {
    fastctx_platform::task::spawn_blocking(work).await.map_err(|error| error.to_string())?.map_err(|error| error.to_string())
}

/// Takes the chunks of an object put, or proved, under its ticket, and
/// says whether the hub took it.
async fn take(hub: &Arc<Hub>, socket: Socket, me: &MachineName, put: Put) -> Result<(), String> {
    let library = scopes::library(&hub.assets, &put.scope)?;
    let pulse = crate::pulse_of(&socket);
    let (sink, mut stream) = socket.split();
    let work = fastctx_net::data::ReceiptWork::default();
    let writer = Writer::new(sink, pulse.clone(), work.clone());
    let Put { object, manifest, missing, prove, .. } = put;
    let holder = format!("upload:{me}");
    // A put of every byte proves holding as well as a proof does.
    let whole = prove || missing.len() as u64 == chunks(object.size);
    let kept: Result<(), String> = async {
        if prove {
            // Every byte, in order, hashed as it comes.
            let mut hasher = Sha256::new();
            let mut length = 0u64;
            for expected in 0..chunks(object.size) {
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
        if object.size as usize <= ROW_BYTES {
            let (_, bytes) = chunk(&mut stream, &pulse).await?;
            let _putting = work.begin();
            let lease = blocking(move || library.put(&mut bytes.as_slice(), &holder, LEASE)).await?;
            if lease.object != object {
                return Err("the bytes sent do not hash to the object".into());
            }
            return Ok(());
        }
        let receiving = library.clone();
        let mut transfer = blocking(move || receiving.receive(object, manifest, None)).await?;
        // Present meanwhile: the chunks still come, and where they are all
        // of it they are hashed here, as no transfer checks them whole.
        let mut hasher = (transfer.is_none() && whole).then(Sha256::new);
        let mut wanted: BTreeSet<u64> = missing.into_iter().collect();
        let mut next = 0;
        while !wanted.is_empty() {
            let (index, bytes) = chunk(&mut stream, &pulse).await?;
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
        if let Some(transfer) = transfer {
            blocking(move || transfer.finish(&holder, LEASE)).await?;
        }
        Ok(())
    }
    .await;
    if kept.is_ok() && whole {
        hub.assets.objects.proofs.lock().expect("proofs").insert((me.clone(), object.hash), Instant::now() + proof_for(object.size));
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

/// Sends the chunks of an object from the chunk `from`, each its head then
/// its bytes, and waits for the member's word that it has them all.
async fn give(hub: &Arc<Hub>, socket: Socket, scope: Scope, object: ObjectRef, from: u64) -> Result<(), String> {
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
        for index in from..chunks(object.size) {
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
