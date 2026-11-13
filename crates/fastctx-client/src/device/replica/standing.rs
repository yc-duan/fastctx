//! Where a device stands in each owner library of a hub, and what it tells
//! the hub of it (design §28.9, §28.40 six): the download half of a node's
//! (`fastctx-node` `assets/session.rs`'s feeds, `assets/standing.rs`). A
//! whole look notes where each owner library stood as it began and, every
//! scope of it gone through, stands there; between looks, what changed in a
//! library it stands in is taken by its change numbers, and it stands at the
//! latest once all of that is taken. Its report says where it stands, the
//! scopes not yet pulled whole and the objects its tier fetches that it
//! lacks: the hub reckons from it whether the device is caught up, exactly.

use super::pass::{ask, hold, read_anew, succession};
use super::{REVOKED, Replica, blocking};
use fastctx_net::ids::HubId;
use fastctx_net::link::Link;
use fastctx_net::sign::PublicKey;
use fastctx_types::assets::{self as wire, Reply, Request};
use fastctx_types::sync_status::{OwnerStanding, SyncReport, UNPULLED_NAMED};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Streams asked after in one page of what changed.
const CHANGES_AT_ONCE: u32 = 256;
/// The classes of streams, in the order they are caught up in after being
/// away: small state first (design §28.1).
const CLASSES: [u8; 4] = [0, 1, 2, 3];
/// The most time between two reports that say the same.
const EVERY: Duration = Duration::from_secs(300);

#[derive(serde::Deserialize)]
struct OwnerOf {
    owner: String,
}

/// The owner library a scope's descriptor names.
pub(super) fn owner_of(descriptor: &[u8]) -> Option<String> {
    serde_json::from_slice::<OwnerOf>(descriptor).ok().map(|read| read.owner)
}

/// Where an owner library stood as it was asked: its generation, its
/// latest change and when that was made.
pub(super) struct Mark {
    owner: String,
    generation: String,
    top: u64,
    at: i64,
}

/// Where each of `owners`' libraries stands now.
pub(super) async fn tops(replica: &Replica, link: &Arc<Link>, owners: &BTreeSet<String>) -> Result<Vec<Mark>, String> {
    let mut marks = Vec::new();
    for owner in owners {
        let request = Request::Changes { owner: owner.clone(), after: 0, before: None, class: None, limit: 1 };
        let Reply::Changes { generation, top, top_at, .. } = ask(replica, link, request).await? else {
            return Err("the hub did not say where its library stands".into());
        };
        marks.push(Mark { owner: owner.clone(), generation, top, at: top_at });
    }
    Ok(marks)
}

/// Stands where each of `marks` says, but in the libraries of `short`,
/// some scope of which a look did not go through whole (II01).
pub(super) async fn stand(replica: &Replica, hub: &HubId, marks: Vec<Mark>, short: &BTreeSet<String>) -> Result<(), String> {
    for mark in marks.into_iter().filter(|mark| !short.contains(&mark.owner) && !mark.generation.is_empty()) {
        let theirs = hub.as_str().to_string();
        blocking(&replica.library, move |library| library.fed(&theirs, &mark.owner, &mark.generation, mark.top, mark.at)).await?;
    }
    Ok(())
}

/// Takes what changed in `owner`'s library of `hub` since the device last
/// stood in it, newest first, a page at a time, class by class when there
/// is more than a page; where it stands moves only once all of it is taken.
/// Whether some stream had more than one turn gave.
pub(super) async fn feed(replica: &Replica, hub: &HubId, link: &Arc<Link>, key: &PublicKey, owner: &str) -> Result<bool, String> {
    let library = &replica.library;
    let stood = {
        let (theirs, owner) = (hub.as_str().to_string(), owner.to_string());
        blocking(library, move |library| library.feed(&theirs, &owner)).await?
    };
    let page = |after: u64, before: Option<u64>, class: Option<u8>| {
        ask(replica, link, Request::Changes { owner: owner.to_string(), after, before, class, limit: CHANGES_AT_ONCE })
    };
    let mut after = stood.as_ref().map_or(0, |(_, change)| *change);
    let Reply::Changes { mut generation, mut streams, mut top, mut top_at, mut more, .. } = page(after, None, None).await? else {
        return Err("the hub did not say what changed".into());
    };
    let renewed = stood.as_ref().is_some_and(|(was, _)| *was != generation);
    if renewed {
        // The hub was put back since: its numbers start again, and a
        // stream it orders may be shorter there than here, which no change
        // number shows; a whole look sets such a stream aside (`pass::pull`).
        replica.call_whole(hub);
        if after > 0 {
            after = 0;
            let Reply::Changes { generation: now, streams: all, top: at, top_at: when, more: further, .. } =
                page(0, None, None).await?
            else {
                return Err("the hub did not say what changed".into());
            };
            (generation, streams, top, top_at, more) = (now, all, at, when, further);
        }
    }
    if generation.is_empty() {
        return Ok(false);
    }
    if top <= after {
        if renewed {
            let (theirs, owner) = (hub.as_str().to_string(), owner.to_string());
            blocking(library, move |library| library.fed(&theirs, &owner, &generation, top, top_at)).await?;
        }
        return Ok(false);
    }
    let theirs = hub.as_str().to_string();
    let unpulled = blocking(library, move |library| library.unpulled_of(&theirs)).await?;
    let mut failed = None;
    let mut went_on = catch_up(replica, hub, link, key, &unpulled, streams, &mut failed).await;
    if more {
        for class in CLASSES {
            let mut before = Some(top + 1);
            loop {
                let Reply::Changes { streams, through, more, .. } = page(after, before, Some(class)).await? else {
                    return Err("the hub did not say what changed".into());
                };
                went_on |= catch_up(replica, hub, link, key, &unpulled, streams, &mut failed).await;
                if !more {
                    break;
                }
                before = Some(through);
            }
        }
    }
    if let Some(why) = failed {
        return Err(why);
    }
    // Not all of it taken this turn: it stands where it stood, and goes on.
    if went_on {
        return Ok(true);
    }
    let (theirs, owner) = (hub.as_str().to_string(), owner.to_string());
    blocking(library, move |library| library.fed(&theirs, &owner, &generation, top, top_at)).await?;
    Ok(false)
}

/// Brings each of `streams`, as the hub said they stand, to where they
/// stand here, as the device's tier holds each. A scope not held here yet,
/// or not pulled whole in its holding period, or a stream first heard of,
/// calls for a whole look instead. Whether some stream had more than one
/// turn gave; the first failure goes into `failed`, the rest gone on with.
async fn catch_up(
    replica: &Replica,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    unpulled: &BTreeSet<String>,
    streams: Vec<wire::ChangedStream>,
    failed: &mut Option<String>,
) -> bool {
    let library = &replica.library;
    let mut went_on = false;
    for changed in streams {
        let done: Result<bool, String> = async {
            let scope = changed.scope.clone();
            let Some(holding) = blocking(library, move |library| library.holding(&scope)).await? else {
                replica.call_whole(hub);
                return Ok(false);
            };
            // Its streams' past shows in no change number after the device's
            // need: a whole look pulls it by itself (design §28.1, E01).
            if unpulled.contains(&changed.scope) {
                replica.call_whole(hub);
                return Ok(false);
            }
            let (scope, stream) = (changed.scope.clone(), changed.stream.clone());
            let known = blocking(library, move |library| library.known_last_epoch(&scope, &stream)).await?;
            // A stream first heard of may stand for another the hub gives the
            // device no more (§29, 2.4): a whole look lists what it withholds.
            if known.is_none() {
                replica.call_whole(hub);
            }
            if known.is_none_or(|known| known < changed.epoch) {
                succession(replica, hub, link, key, &changed.scope, &changed.stream).await?;
            }
            let scope = changed.scope.clone();
            let at = blocking(library, move |library| library.streams(&scope))
                .await?
                .into_iter()
                .find(|(name, _)| *name == changed.stream)
                .map_or(0, |(_, next)| next);
            let anew = read_anew(replica, &changed.scope, &changed.stream, changed.remade, changed.next).await?;
            hold(replica, hub, link, key, &changed.scope, &changed.stream, holding, at, changed.next, anew).await
        }
        .await;
        match done {
            Ok(more) => went_on |= more,
            Err(why) if why.starts_with(REVOKED) => {}
            Err(why) => {
                failed.get_or_insert(format!("{} of {}: {why}", changed.stream, changed.scope));
            }
        }
    }
    went_on
}

/// Tells `hub` where the device stands when that says something new, and
/// at least every five minutes: a device writes nothing, so it has nothing
/// to send, and it counts only what its tier fetches as missing.
pub(super) async fn report(replica: &Replica, hub: &HubId, link: &Arc<Link>) -> Result<(), String> {
    let at_most = super::tiers::fetched_at_most(replica.tier());
    let theirs = hub.as_str().to_string();
    let (feeds, unpulled, lacking) = blocking(&replica.library, move |library| {
        Ok((library.feeds_of(&theirs)?, library.unpulled_of(&theirs)?, library.lacking_within(&theirs, at_most)?))
    })
    .await?;
    let report = SyncReport {
        owners: feeds
            .into_iter()
            .map(|fed| OwnerStanding { owner: fed.owner, generation: fed.generation, change: fed.change, at: fed.at, taken: 0 })
            .collect(),
        unpulled: u32::try_from(unpulled.len()).unwrap_or(u32::MAX),
        unpulled_scopes: unpulled.into_iter().take(UNPULLED_NAMED).collect(),
        missing_objects: lacking.0,
        missing_bytes: lacking.1,
        unsent_frames: 0,
        unsent_bytes: 0,
        rate: 0,
    };
    let due = replica.reported.lock().expect("reported").get(hub).is_none_or(|(last, at)| *last != report || at.elapsed() >= EVERY);
    if !due {
        return Ok(());
    }
    let Reply::Reported = ask(replica, link, Request::Report { report: report.clone() }).await? else {
        return Err("the hub did not keep the report".into());
    };
    replica.reported.lock().expect("reported").insert(hub.clone(), (report, Instant::now()));
    Ok(())
}
