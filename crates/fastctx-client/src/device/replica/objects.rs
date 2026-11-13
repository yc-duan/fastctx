//! Objects a device's records refer to and it has not, fetched from the hub
//! (design §6.5, §7.3) as a node fetches them: each in chunks over a data
//! connection under a ticket the hub gives, a fetch broken off taken up
//! where its chunks stopped. A light device fetches the short ones as their
//! records come and the rest as they are read (§29 3.3).

use super::{Replica, blocking};
use fastctx_content::ObjectRef;
use fastctx_content::v2::{CHUNK_BYTES, LEASE, Library, ROW_BYTES};
use fastctx_net::ids::HubId;
use fastctx_net::link::Link;
use fastctx_types::assets::{self as wire, Reply, Request};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Objects fetched in one pass.
const FETCHED_AT_ONCE: usize = 64;
/// How long an object a hub did not give waits before it is asked of that
/// hub again, longer each time it was not.
const ASK_AGAIN: Duration = Duration::from_secs(60);

fn chunks(size: u64) -> u64 {
    size.div_ceil(CHUNK_BYTES).max(1)
}

/// Fetches what the records taken refer to and this device has not, of the
/// length its tier fetches as they come; what did not come, and why, and
/// whether more may be waiting than one look fetched.
pub(super) async fn fetch(replica: &Replica, hub: &HubId, link: &Arc<Link>) -> (Vec<String>, bool) {
    let library = &replica.library;
    let at_most = super::tiers::fetched_at_most(replica.tier());
    let skipped: Vec<[u8; 32]> = {
        let unreached = replica.unreached.lock().expect("unreached");
        let deferred = replica.deferred.lock().expect("deferred");
        unreached
            .iter()
            .filter(|((of, _), (at, failed))| of == hub && at.elapsed() < ASK_AGAIN * 2u32.pow((*failed).min(6)))
            .map(|((_, hash), _)| *hash)
            .chain(deferred.iter().copied())
            .collect()
    };
    let theirs = hub.as_str().to_string();
    let wanted = match blocking(library, move |library| library.wanted_of_hub(&theirs, FETCHED_AT_ONCE, &skipped)).await {
        Ok(wanted) => wanted,
        Err(why) => return (vec![format!("listing the objects wanted: {why}")], false),
    };
    let more = wanted.len() == FETCHED_AT_ONCE;
    let mut failed = Vec::new();
    for (scope, object) in wanted {
        if object.size > at_most {
            // Fetched as it is read (`Replica::fetch`).
            replica.deferred.lock().expect("deferred").insert(object.hash);
            continue;
        }
        if let Err(why) = fetch_now(replica, hub, link, &scope, object).await {
            failed.push(format!("fetching an object of {scope}: {why}"));
        }
    }
    (failed, more)
}

/// Fetches `object` of `scope` from the hub, remembering a failure.
pub(super) async fn fetch_now(replica: &Replica, hub: &HubId, link: &Arc<Link>, scope: &str, object: ObjectRef) -> Result<(), String> {
    let library = &replica.library;
    match fetch_one(replica, link, scope, object).await {
        Ok(()) => {
            replica.unreached.lock().expect("unreached").remove(&(hub.clone(), object.hash));
            replica.deferred.lock().expect("deferred").remove(&object.hash);
            let hash = object.hash;
            blocking(library, move |library| library.unwant(&hash)).await
        }
        Err(why) => {
            let mut unreached = replica.unreached.lock().expect("unreached");
            let times = unreached.get(&(hub.clone(), object.hash)).map_or(0, |(_, times)| times + 1);
            unreached.insert((hub.clone(), object.hash), (Instant::now(), times));
            Err(why)
        }
    }
}

/// Fetches `object`, asked of the hub as every request of a scope is: one
/// the device reads no more ends its holding here (`pass::ask`).
async fn fetch_one(replica: &Replica, link: &Arc<Link>, scope: &str, object: ObjectRef) -> Result<(), String> {
    let library: &Library = &replica.library;
    let holder = format!("fetch:{scope}");
    let wire_object = wire::ObjectRef { hash: object.hash.into(), size: object.size };
    if object.size as usize <= ROW_BYTES {
        let request = Request::Get { scope: scope.to_string(), object: wire_object, from: 0 };
        let Reply::Get { ticket, .. } = super::pass::ask(replica, link, request).await? else {
            return Err("the hub did not give the object".into());
        };
        let mut incoming = link.get_object(ticket).await?;
        let (_, bytes) = incoming.chunk().await?;
        incoming.done().await?;
        let lease = blocking(library, move |library| library.put(&mut bytes.as_slice(), &holder, LEASE)).await?;
        if lease.object != object {
            return Err("the bytes given do not hash to the object".into());
        }
        return blocking(library, move |library| library.release(lease)).await;
    }
    // Takes up a fetch broken off, from the first chunk not here.
    let transfer = blocking(library, move |library| library.under_way(object)).await?;
    let mut from = transfer.as_ref().and_then(|transfer| transfer.missing().first().copied()).unwrap_or(0);
    let request = Request::Get { scope: scope.to_string(), object: wire_object.clone(), from };
    let Reply::Get { ticket, .. } = super::pass::ask(replica, link, request).await? else {
        return Err("the hub did not give the object".into());
    };
    let mut incoming = link.get_object(ticket).await?;
    let digests = incoming.manifest(chunks(object.size)).await?;
    let resumed = transfer.is_some();
    let Some(started) = blocking(library, move |library| library.receive(object, digests, Some("hub".into()))).await? else {
        return incoming.done().await;
    };
    if resumed && from > 0 && started.missing().first().copied().unwrap_or(0) < from {
        // The hub's digests are not those the fetch began under: the chunks
        // come again from the first.
        drop(incoming);
        from = 0;
        let request = Request::Get { scope: scope.to_string(), object: wire_object, from };
        let Reply::Get { ticket: again, .. } = super::pass::ask(replica, link, request).await? else {
            return Err("the hub did not give the object".into());
        };
        incoming = link.get_object(again).await?;
        incoming.manifest(chunks(object.size)).await?;
    }
    let mut transfer = Some(started);
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
    incoming.done().await?;
    let finishing = transfer.take().expect("a transfer");
    let lease = fastctx_platform::task::spawn_blocking(move || finishing.finish(&holder, LEASE))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    blocking(library, move |library| library.release(lease)).await
}
