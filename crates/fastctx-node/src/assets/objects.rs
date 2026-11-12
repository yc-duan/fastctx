//! Objects between this machine and a hub (design §6.5, §7.3): those a
//! frame it uploads refers to, put when the hub has them not or wants them
//! proved by their bytes, and those the records it took refer to and it has
//! not, fetched. Each travels in chunks over a data connection under a
//! ticket the hub gives on the control connection; a fetch broken off takes
//! up where its chunks stopped.

use super::Assets;
use super::session::blocking;
use fastctx_content::ObjectRef;
use fastctx_content::v2::{CHUNK_BYTES, LEASE, Library, ROW_BYTES};
use fastctx_net::link::Link;
use fastctx_types::assets::{self as wire, Bytes, Reply, Request};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Objects fetched in one pass.
const FETCHED_AT_ONCE: usize = 64;
/// How long an object a hub did not give waits before it is asked of that
/// hub again.
const ASK_AGAIN: Duration = Duration::from_secs(60);

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
/// `prove`; their hashes, to name as proven when the frame goes again.
pub(super) async fn put(library: &Library, link: &Arc<Link>, scope: &str, objects: &[wire::ObjectRef], prove: bool) -> Result<Vec<Bytes>, String> {
    let mut proven = Vec::with_capacity(objects.len());
    for object in objects {
        let object = local(object)?;
        let hash = object.hash;
        if blocking(library, move |library| library.size(&hash)).await? != Some(object.size) {
            return Err(format!("a frame refers to the object {} this machine does not hold", hex(&hash)));
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
            outgoing.finish().await?;
        }
        proven.push(Bytes::from(hash));
    }
    Ok(proven)
}

/// Fetches what the records taken refer to and this machine has not; what
/// did not come, and why.
pub(super) async fn fetch(assets: &Assets, hub: &fastctx_types::access::HubId, link: &Arc<Link>) -> Vec<String> {
    let library = &assets.library;
    // What this hub described alone is asked of it.
    let mine = match super::session::hub_scopes(library, hub).await {
        Ok(mine) => mine,
        Err(why) => return vec![format!("listing the hub's scopes: {why}")],
    };
    let waiting: std::collections::HashSet<[u8; 32]> = {
        let mut unreached = assets.unreached.lock().expect("unreached");
        unreached.retain(|_, at| at.elapsed() < ASK_AGAIN);
        unreached.keys().filter(|(of, _)| of == hub).map(|(_, hash)| *hash).collect()
    };
    let limit = FETCHED_AT_ONCE + waiting.len();
    let scopes: Vec<String> = mine.iter().cloned().collect();
    let wanted = match blocking(library, move |library| library.wanted_of(&scopes, limit)).await {
        Ok(wanted) => wanted.into_iter().map(|(scope, object)| (Some(scope), object)).collect::<Vec<_>>(),
        Err(why) => return vec![format!("listing the objects wanted: {why}")],
    };
    let mut failed = Vec::new();
    for (scope, object) in wanted.into_iter().filter(|(_, object)| !waiting.contains(&object.hash)).take(FETCHED_AT_ONCE) {
        // Only a scope names where an object is read, and only its hub is
        // asked.
        let Some(scope) = scope.filter(|scope| mine.contains(scope)) else { continue };
        match fetch_one(library, link, &scope, object).await {
            Ok(()) => {
                let hash = object.hash;
                if let Err(why) = blocking(library, move |library| library.unwant(&hash)).await {
                    failed.push(format!("the object {}: {why}", hex(&object.hash)));
                }
            }
            Err(why) => {
                assets.unreached.lock().expect("unreached").insert((hub.clone(), object.hash), Instant::now());
                failed.push(format!("fetching the object {} of {scope}: {why}", hex(&object.hash)));
            }
        }
    }
    failed
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
        incoming.done().await?;
        let lease = blocking(library, move |library| library.put(&mut bytes.as_slice(), &holder, LEASE)).await?;
        if lease.object != object {
            return Err("the bytes given do not hash to the object".into());
        }
        return Ok(());
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
    let Some(started) = started else { return incoming.done().await };
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
    }
    // Every byte is here and written: the hub hears so before the whole is
    // read through again here, however long that takes.
    incoming.done().await?;
    let finishing = transfer.take().expect("a transfer");
    fastctx_platform::task::spawn_blocking(move || finishing.finish(&holder, LEASE))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(())
}
