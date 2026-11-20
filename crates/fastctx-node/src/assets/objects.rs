//! Objects between this machine and a hub (design §6.5, §7.3): those a
//! frame it uploads refers to, put when the hub has them not or wants them
//! proved by their bytes, and those the records it took refer to and it has
//! not, fetched. With a hub that keeps asset lines they go on those, many
//! asked for at once and several lines at a time, off the passes
//! (`lane`); a longer one, and every one with a hub that keeps none,
//! travels in chunks over a data connection under a ticket the hub gives on
//! the control connection, a fetch broken off taking up where its chunks
//! stopped.

use super::Assets;
use super::session::blocking;
use crate::Node;
use fastctx_content::ObjectRef;
use fastctx_content::v2::{CHUNK_BYTES, LEASE, Library, ROW_BYTES};
use fastctx_net::link::{Given, Link, PutAsked};
use fastctx_types::access::HubId;
use fastctx_types::asset_lines::{OBJECT_BYTES, OBJECTS};
use fastctx_types::assets::{self as wire, Bytes, Reply, Request};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Objects fetched in one pass, under tickets.
const FETCHED_AT_ONCE: usize = 64;
/// Objects asked for in one round on lines.
const FETCHED_A_ROUND: usize = 384;
/// Lines objects come on at once: the others are left to frames.
const LINES_FETCHING: usize = 3;
/// The most bytes of objects one ask on a line names, so that no exchange
/// keeps a line long.
const ASKED_BYTES: u64 = 16 << 20;
/// How long an object a hub did not give waits before it is asked of that
/// hub again.
const ASK_AGAIN: Duration = Duration::from_secs(60);
/// How long an object's data connection is waited for before the node says
/// it waits (`doing::waiting_outside`): a label, not a bound.
const SENT_WITHIN: Duration = Duration::from_secs(30);

fn chunks(size: u64) -> u64 {
    size.div_ceil(CHUNK_BYTES).max(1)
}

fn local(object: &wire::ObjectRef) -> Result<ObjectRef, String> {
    Ok(ObjectRef { hash: object.hash.array().ok_or("an object's hash is 32 bytes")?, size: object.size })
}

fn hex(hash: &[u8; 32]) -> String {
    hash.iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

/// Puts each of `objects` of `scope` to the hub, every byte of each when
/// `prove` — or, with a hub that speaks it, an answer to a challenge for it
/// (design §28.45); their hashes, to name as proven when the frame goes
/// again.
pub(super) async fn put(library: &Library, link: &Arc<Link>, scope: &str, objects: &[wire::ObjectRef], prove: bool) -> Result<Vec<Bytes>, String> {
    let mut proven = Vec::with_capacity(objects.len());
    let mut held = Vec::with_capacity(objects.len());
    for object in objects {
        let object = local(object)?;
        let hash = object.hash;
        if blocking(library, move |library| library.size(&hash)).await? != Some(object.size) {
            return Err(format!("a frame refers to the object {} this machine does not hold", hex(&hash)));
        }
        held.push(object);
    }
    // With a hub that speaks it, each proved by a challenge, two at a time
    // (design §28.45); what one leaves is proved by sending it whole.
    let mut answered: std::collections::HashMap<[u8; 32], Answered> = std::collections::HashMap::new();
    if prove && link.asset_version() >= wire::CHALLENGES {
        let mut waiting = held.iter().copied();
        let mut running = tokio::task::JoinSet::new();
        loop {
            while running.len() < PROVED_AT_ONCE {
                let Some(object) = waiting.next() else { break };
                let (library, link) = (library.clone(), link.clone());
                running.spawn(async move { (object.hash, by_challenge(&library, &link, object).await) });
            }
            let Some(done) = running.join_next().await else { break };
            if let Ok((hash, outcome)) = done {
                answered.insert(hash, outcome);
            }
        }
    }
    for object in held {
        let hash = object.hash;
        match answered.remove(&hash) {
            Some(Answered::Proven) => {
                proven.push(Bytes::from(hash));
                continue;
            }
            Some(Answered::Again(why)) => return Err(format!("the frame goes again, the hub to say what it wants: {why}")),
            Some(Answered::Send) | None => {}
        }
        if link.keeps_asset_lines() && object.size <= OBJECT_BYTES {
            put_on_line(library, link, scope, object, prove).await?;
            proven.push(Bytes::from(hash));
            continue;
        }
        let large = object.size as usize > ROW_BYTES;
        let request = Request::Put {
            scope: scope.to_string(),
            object: wire::ObjectRef { hash: hash.into(), size: object.size },
            manifest: Vec::new(),
            prove,
        };
        let Reply::Put { ticket, missing } = link.assets(request).await? else {
            return Err("the hub did not say what of the object it wants".into());
        };
        if ticket != 0 {
            // Waited for past their time, the node says which: the data
            // connection's own bounds are longer (§28.40, the stuck pass).
            let _sending = fastctx_platform::doing::waiting_outside(format!("an object's data connection to the hub ({} bytes)", object.size), SENT_WITHIN);
            let mut outgoing = link.put_object(ticket).await?;
            // A large object's digests go first on the data connection; the
            // hub says which chunks it wants.
            let missing: Vec<u64> = if prove {
                // A proof is every chunk, in order.
                (0..chunks(object.size)).collect()
            } else if large {
                let digests = blocking(library, move |library| library.chunks(&hash)).await?;
                outgoing.manifest(&digests).await?.into_iter().flat_map(|(start, end)| start..end).collect()
            } else {
                missing
            };
            for index in missing {
                if index >= chunks(object.size) {
                    return Err(format!("the hub asked for chunk {index} of an object of {} bytes", object.size));
                }
                let offset = index * CHUNK_BYTES;
                let length = (object.size - offset).min(CHUNK_BYTES) as u32;
                let bytes = blocking(library, move |library| library.read(&hash, offset, length)).await?;
                outgoing.chunk(index, &bytes).await?;
            }
            let _received = fastctx_platform::doing::waiting_outside("the hub's word that it has the object", SENT_WITHIN);
            outgoing.finish().await?;
        }
        proven.push(Bytes::from(hash));
    }
    Ok(proven)
}

/// Puts `object` of `scope` on an asset line, every byte of it when the
/// hub wants it or `prove`.
async fn put_on_line(library: &Library, link: &Arc<Link>, scope: &str, object: ObjectRef, prove: bool) -> Result<(), String> {
    let _sending = fastctx_platform::doing::waiting_outside(format!("an object going to the hub on an asset line ({} bytes)", object.size), SENT_WITHIN);
    let mut line = link.asset_line().await?;
    let named = wire::ObjectRef { hash: object.hash.into(), size: object.size };
    match line.put(scope, &named, prove).await? {
        PutAsked::Held => Ok(()),
        PutAsked::Reply(_) => Err("the hub did not say what of the object it wants".into()),
        PutAsked::Send => {
            let hash = object.hash;
            let mut offset = 0;
            while offset < object.size {
                let length = fastctx_types::asset_lines::RUN_BYTES.min(object.size - offset) as u32;
                let run = blocking(library, move |library| library.read(&hash, offset, length)).await?;
                if run.is_empty() {
                    return Err("the object ended before its length".into());
                }
                line.put_run(&run).await?;
                offset += run.len() as u64;
            }
            line.put_done().await
        }
    }
}

/// How often a proof being worked out by the hub is asked after.
const PROOF_ASKED_EVERY: Duration = Duration::from_millis(250);
/// How often the hub is told, while this machine reads an object to answer
/// a challenge, to keep what it proved and staged.
const KEEP_EVERY: Duration = Duration::from_secs(30);
/// Objects proved by challenge at once, as the hub works two out at once.
const PROVED_AT_ONCE: usize = 2;
/// Replies lost in a row before a proof by challenge is given up.
const LOST_MOST: u32 = 3;

/// How proving an object by a challenge ended (design §28.45).
enum Answered {
    /// The hub took it as proved.
    Proven,
    /// The hub holds it no more, found its own bytes damaged, or knows the
    /// challenge no more: the frame goes again, the hub then saying what it
    /// wants of it.
    Again(String),
    /// Proved by sending every byte instead: the hub would not challenge,
    /// or the answer was not its.
    Send,
}

/// Proves to the hub that this machine holds `object` by answering a
/// challenge (design §28.45). While it reads the object the hub is told to
/// keep what it proved and staged; a reply lost is asked again; how long it
/// asks after goes by this machine's own clock, as long as the hub may take
/// to read the object.
async fn by_challenge(library: &Library, link: &Arc<Link>, object: ObjectRef) -> Answered {
    let challenge = Request::Challenge { object: wire::ObjectRef { hash: object.hash.into(), size: object.size } };
    let nonce = match link.assets(challenge).await {
        Ok(Reply::Challenge { nonce, .. }) => nonce,
        Err(why) if why.contains("send the frame again") => return Answered::Again(why),
        _ => return Answered::Send,
    };
    let Some(given) = nonce.array() else { return Answered::Send };
    let keeping = {
        let (link, nonce) = (link.clone(), nonce.clone());
        fastctx_platform::task::spawn(async move {
            loop {
                tokio::time::sleep(KEEP_EVERY).await;
                let _ = link.assets(Request::Proof { nonce: nonce.clone(), answer: Bytes(Vec::new()) }).await;
            }
        })
    };
    let hash = object.hash;
    let answer = blocking(library, move |library| answer_of(library, &hash, object.size, &given)).await;
    keeping.abort();
    let Ok(answer) = answer else { return Answered::Send };
    let asked_until = Instant::now() + Duration::from_secs(600 + object.size / (50 << 20) + 60);
    let mut lost = 0;
    loop {
        let asking = Request::Proof { nonce: nonce.clone(), answer: Bytes(answer.to_vec()) };
        match link.assets(asking).await {
            Ok(Reply::Proof { proof: wire::ProofState::Proven }) => return Answered::Proven,
            Ok(Reply::Proof { proof: wire::ProofState::Working }) if Instant::now() < asked_until => {
                lost = 0;
                tokio::time::sleep(PROOF_ASKED_EVERY).await;
            }
            Ok(Reply::Proof { proof: wire::ProofState::Damaged }) => {
                return Answered::Again("the hub's own copy of the object was damaged; it is wanted whole again".into());
            }
            Ok(_) => return Answered::Send,
            Err(why) if why.contains("send the frame again") => return Answered::Again(why),
            Err(_) if lost < LOST_MOST && Instant::now() < asked_until => {
                lost += 1;
                tokio::time::sleep(PROOF_ASKED_EVERY * 4).await;
            }
            Err(_) => return Answered::Send,
        }
    }
}

/// `SHA256(PROOF_TAG ‖ nonce ‖ every byte)` of the object `hash` of `size`
/// held here, read a chunk at a time.
fn answer_of(library: &Library, hash: &[u8; 32], size: u64, nonce: &[u8; 32]) -> fastctx_content::Result<[u8; 32]> {
    use sha2::Digest as _;
    let mut digest = sha2::Sha256::new();
    digest.update(wire::PROOF_TAG);
    digest.update(nonce);
    let mut offset = 0;
    while offset < size {
        let length = (size - offset).min(CHUNK_BYTES) as u32;
        let bytes = library.read(hash, offset, length)?;
        if bytes.is_empty() {
            return Err(fastctx_content::Error::Invalid("the object ended before its length".into()));
        }
        digest.update(&bytes);
        offset += bytes.len() as u64;
    }
    Ok(digest.finalize().into())
}

/// The objects of `hub`'s scopes wanted now, at most `limit`: those the
/// hub did not give lately wait, longer each time it did not, and never
/// fill the place of those not yet asked.
async fn wanted(assets: &Assets, hub: &HubId, limit: usize) -> Result<Vec<(String, ObjectRef)>, String> {
    let waiting: Vec<[u8; 32]> = {
        let unreached = assets.unreached.lock().expect("unreached");
        unreached
            .iter()
            .filter(|((of, _), (at, failed))| of == hub && at.elapsed() < ASK_AGAIN * 2u32.pow((*failed).min(6)))
            .map(|((_, hash), _)| *hash)
            .collect()
    };
    // What this hub's network's scopes want alone is asked of it, in one
    // look however many scopes it has; of those this machine takes nothing
    // of in now (`copies`), nothing.
    let theirs = hub.as_str().to_string();
    let paused: Vec<String> = assets.copies.paused_now().iter().cloned().collect();
    blocking(&assets.library, move |library| library.wanted_of_hub_but(&theirs, limit, &waiting, &paused)).await
}

/// The hub gave `object`.
fn reached(assets: &Assets, hub: &HubId, object: &ObjectRef) {
    assets.unreached.lock().expect("unreached").remove(&(hub.clone(), object.hash));
}

/// The hub did not give `object`: it waits before it is asked for again.
fn unreached(assets: &Assets, hub: &HubId, object: &ObjectRef) {
    let mut unreached = assets.unreached.lock().expect("unreached");
    let times = unreached.get(&(hub.clone(), object.hash)).map_or(0, |(_, times)| times + 1);
    unreached.insert((hub.clone(), object.hash), (Instant::now(), times));
}

/// Fetches what the records taken refer to and this machine has not, under
/// tickets, one at a time; what did not come, and why.
pub(super) async fn fetch(assets: &Assets, hub: &HubId, link: &Arc<Link>) -> Vec<String> {
    let library = &assets.library;
    let wanted = match wanted(assets, hub, FETCHED_AT_ONCE).await {
        Ok(wanted) => wanted,
        Err(why) => return vec![format!("listing the objects wanted: {why}")],
    };
    let mut failed = Vec::new();
    for (scope, object) in wanted {
        if let Err(why) = fetch_alone(assets, hub, link, &scope, object).await {
            failed.push(why);
        }
    }
    let _ = library;
    failed
}

/// Fetches `object` of `scope` under a ticket, and keeps whether the hub
/// gave it.
async fn fetch_alone(assets: &Assets, hub: &HubId, link: &Arc<Link>, scope: &str, object: ObjectRef) -> Result<(), String> {
    let library = &assets.library;
    // Kept as it comes: this machine clears nothing until it is all here.
    let _keeping = assets.clearing.read().await;
    // Paused since it was listed, it is not asked for (`copies`).
    if assets.copies.paused_now().contains(scope) {
        return Ok(());
    }
    match fetch_one(library, link, scope, object).await {
        Ok(()) => {
            reached(assets, hub, &object);
            let hash = object.hash;
            blocking(library, move |library| library.unwant(&hash)).await.map_err(|why| format!("the object {}: {why}", hex(&object.hash)))
        }
        Err(why) => {
            unreached(assets, hub, &object);
            Err(format!("fetching the object {} of {scope}: {why}", hex(&object.hash)))
        }
    }
}

/// Fetches the objects `hub`'s scopes want off the passes, a round after
/// another until none it gives is left, where none is fetched so already:
/// the frames of the streams go on meanwhile, and a pass waits for no
/// object. Where this machine stands is told the hub again once some came.
pub(super) fn lane(node: &Arc<Node>, hub: &HubId) {
    let Some(assets) = node.assets.as_ref() else { return };
    if !assets.fetching.lock().expect("fetching").insert(hub.clone()) {
        return;
    }
    let (weak, hub) = (Arc::downgrade(node), hub.clone());
    fastctx_platform::task::spawn(async move {
        let _doing = fastctx_platform::doing::doing(format!("fetching objects from {hub}"));
        let mut came = false;
        loop {
            let Some(node) = weak.upgrade() else { return };
            let Some(link) = node.link_of(&hub).filter(|link| link.keeps_asset_lines()) else { break };
            let (fetched, failed) = round(&node, &hub, &link).await;
            for failed in failed {
                node.log.line(format_args!("assets with {hub}: {failed}; asked again later"));
            }
            came |= fetched > 0;
            if fetched == 0 {
                break;
            }
        }
        let Some(node) = weak.upgrade() else { return };
        if let Some(assets) = node.assets.as_ref() {
            assets.fetching.lock().expect("fetching").remove(&hub);
        }
        if came {
            super::wake(&node, &hub);
        }
    });
}

/// One round of fetching on lines: the objects wanted asked for scope by
/// scope, as many to an ask as a line carries, several lines at once; how
/// many came, and what did not and why.
async fn round(node: &Arc<Node>, hub: &HubId, link: &Arc<Link>) -> (usize, Vec<String>) {
    let Some(assets) = node.assets.as_ref() else { return (0, Vec::new()) };
    let wanted = match wanted(assets, hub, FETCHED_A_ROUND).await {
        Ok(wanted) => wanted,
        Err(why) => return (0, vec![format!("listing the objects wanted: {why}")]),
    };
    // Those of one scope go in one ask, up to what an ask carries; one too
    // long for a line goes alone, under a ticket.
    let mut asks: Vec<(String, Vec<ObjectRef>)> = Vec::new();
    let mut alone = Vec::new();
    for (scope, object) in wanted {
        if object.size > OBJECT_BYTES {
            alone.push((scope, object));
            continue;
        }
        match asks.iter_mut().rev().find(|(of, objects)| {
            *of == scope && objects.len() < OBJECTS && objects.iter().map(|object| object.size).sum::<u64>() + object.size <= ASKED_BYTES
        }) {
            Some((_, objects)) => objects.push(object),
            None => asks.push((scope, vec![object])),
        }
    }
    let room = Arc::new(tokio::sync::Semaphore::new(LINES_FETCHING));
    let mut fetching = Vec::with_capacity(asks.len());
    for (scope, objects) in asks {
        let (node, hub, link, room) = (node.clone(), hub.clone(), link.clone(), room.clone());
        fetching.push(fastctx_platform::task::spawn(async move {
            let _room = room.acquire_owned().await;
            let Some(assets) = node.assets.as_ref() else { return (0, Vec::new()) };
            asked(assets, &hub, &link, &scope, objects).await
        }));
    }
    let (mut fetched, mut failed) = (0, Vec::new());
    for (scope, object) in alone {
        match fetch_alone(assets, hub, link, &scope, object).await {
            Ok(()) => fetched += 1,
            Err(why) => failed.push(why),
        }
    }
    for fetching in fetching {
        if let Ok((came, why)) = fetching.await {
            fetched += came;
            failed.extend(why);
        }
    }
    (fetched, failed)
}

/// Asks for `objects` of `scope` on one line and keeps each as it comes;
/// how many came, and what did not and why. A line that breaks leaves the
/// rest to be asked for again at once, as none of them was refused.
async fn asked(assets: &Assets, hub: &HubId, link: &Arc<Link>, scope: &str, objects: Vec<ObjectRef>) -> (usize, Vec<String>) {
    // Paused since it was listed, nothing of it is asked for (`copies`).
    if assets.copies.paused_now().contains(scope) {
        return (0, Vec::new());
    }
    let library = &assets.library;
    let named: Vec<wire::ObjectRef> = objects.iter().map(|object| wire::ObjectRef { hash: object.hash.into(), size: object.size }).collect();
    let _coming = fastctx_platform::doing::waiting_outside(format!("{} objects of {scope} coming from the hub on an asset line", objects.len()), SENT_WITHIN);
    let (mut fetched, mut failed) = (0, Vec::new());
    // What is asked for now is kept only while this machine has not begun
    // since to clear what another account may not see.
    let cleared = assets.cleared_times.load(std::sync::atomic::Ordering::SeqCst);
    let mut line = match link.asset_line().await {
        Ok(line) => line,
        Err(why) => return (0, vec![format!("an asset line for the objects of {scope}: {why}")]),
    };
    // Paused while a line was waited for, nothing is asked for.
    if assets.copies.paused_now().contains(scope) {
        return (0, Vec::new());
    }
    if let Err(why) = line.ask_objects(scope, &named).await {
        return (0, vec![format!("asking for the objects of {scope}: {why}")]);
    }
    for (index, object) in objects.iter().enumerate() {
        let given = match line.object(&named[index], index, index + 1 == objects.len()).await {
            Ok(given) => given,
            Err(why) => {
                failed.push(format!("fetching the objects of {scope}: {why}"));
                break;
            }
        };
        match given {
            Given::Bytes(bytes) => {
                let _keeping = assets.clearing.read().await;
                if assets.cleared_times.load(std::sync::atomic::Ordering::SeqCst) != cleared {
                    failed.push(format!("the objects of {scope} were asked for before this machine cleared what it held"));
                    break;
                }
                let (holder, wanted) = (format!("fetch:{scope}"), *object);
                // Kept under a lease only until it is seen to be the object:
                // the records that want it hold it from then on.
                let kept = blocking(library, move |library| {
                    let lease = library.put(&mut bytes.as_slice(), &holder, LEASE)?;
                    if lease.object != wanted {
                        return Err(fastctx_content::Error::Invalid("the bytes given do not hash to the object".into()));
                    }
                    library.release(lease)?;
                    library.unwant(&wanted.hash)
                })
                .await;
                match kept {
                    Ok(()) => {
                        reached(assets, hub, object);
                        fetched += 1;
                    }
                    Err(why) => {
                        unreached(assets, hub, object);
                        failed.push(format!("keeping the object {} of {scope}: {why}", hex(&object.hash)));
                    }
                }
            }
            Given::Withheld(why) => {
                unreached(assets, hub, object);
                failed.push(format!("fetching the object {} of {scope}: {why}", hex(&object.hash)));
            }
            Given::Refused(answer) => {
                drop(line);
                // A scope revoked ends its holding here, as any answer of it does.
                let why = match answer {
                    Ok(reply) => match super::session::heard(assets, link, reply).await {
                        Ok(_) => "the hub gave none of them".to_string(),
                        Err(why) => why,
                    },
                    Err(why) => why,
                };
                for object in &objects {
                    unreached(assets, hub, object);
                }
                failed.push(format!("fetching the objects of {scope}: {why}"));
                return (fetched, failed);
            }
        }
    }
    (fetched, failed)
}

/// Tells the hub every chunk of an object came. The hub lets the line go as
/// it hears it, so the line may be seen to end before the telling is: what
/// came stands whatever that says, proved by its hash as it is kept, and is
/// never thrown away and asked for again only after a wait.
async fn said_done(incoming: fastctx_net::link::Incoming) {
    let _ = incoming.done().await;
}

async fn fetch_one(library: &Library, link: &Arc<Link>, scope: &str, object: ObjectRef) -> Result<(), String> {
    let holder = format!("fetch:{scope}");
    let wire_object = wire::ObjectRef { hash: object.hash.into(), size: object.size };
    if object.size as usize <= ROW_BYTES {
        let request = Request::Get { scope: scope.to_string(), object: wire_object, from: 0 };
        let Reply::Get { ticket, .. } = link.assets(request).await? else {
            return Err("the hub did not give the object".into());
        };
        let mut incoming = link.get_object(ticket).await?;
        let (_, bytes) = incoming.chunk().await?;
        said_done(incoming).await;
        let lease = blocking(library, move |library| library.put(&mut bytes.as_slice(), &holder, LEASE)).await?;
        if lease.object != object {
            return Err("the bytes given do not hash to the object".into());
        }
        // The records that want it hold it now: no lease keeps it beyond them.
        return blocking(library, move |library| library.release(lease)).await;
    }
    // Takes up a fetch broken off, from the first chunk not here.
    let mut transfer = blocking(library, move |library| library.under_way(object)).await?;
    let mut from = transfer.as_ref().and_then(|transfer| transfer.missing().first().copied()).unwrap_or(0);
    let request = Request::Get { scope: scope.to_string(), object: wire_object.clone(), from };
    let Reply::Get { ticket, .. } = link.assets(request).await? else {
        return Err("the hub did not give the object".into());
    };
    let mut incoming = link.get_object(ticket).await?;
    let digests = incoming.manifest(chunks(object.size)).await?;
    let resumed = transfer.is_some();
    let started = blocking(library, move |library| library.receive(object, digests, Some("hub".into()))).await?;
    let Some(started) = started else {
        said_done(incoming).await;
        return Ok(());
    };
    if resumed && from > 0 && started.missing().first().copied().unwrap_or(0) < from {
        // The hub's digests are not those the fetch began under: what came
        // counts for nothing, and the chunks come again from the first.
        drop(incoming);
        from = 0;
        let request = Request::Get { scope: scope.to_string(), object: wire_object, from };
        let Reply::Get { ticket: again, .. } = link.assets(request).await? else {
            return Err("the hub did not give the object".into());
        };
        incoming = link.get_object(again).await?;
        incoming.manifest(chunks(object.size)).await?;
    }
    transfer = Some(started);
    for expected in from..chunks(object.size) {
        let (index, bytes) = incoming.chunk().await?;
        if index != expected {
            return Err(format!("chunk {index} came where {expected} was due"));
        }
        let mut writing = transfer.take().expect("a transfer");
        transfer = Some(
            fastctx_platform::task::spawn_blocking(move || writing.chunk(index, &bytes).map(|()| writing))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?,
        );
        fastctx_platform::lab::die_at("chunk", scope);
    }
    // Every byte is here and written: the hub hears so before the whole is
    // read through again here, however long that takes.
    said_done(incoming).await;
    let finishing = transfer.take().expect("a transfer");
    let lease = fastctx_platform::task::spawn_blocking(move || finishing.finish(&holder, LEASE))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    // The records that want it hold it now: no lease keeps it beyond them.
    blocking(library, move |library| library.release(lease)).await
}
