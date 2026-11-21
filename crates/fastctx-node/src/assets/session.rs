//! One asset sync driver per hub (design §6.5, §6.6, §7.4, §7.5, §17): each
//! pass opens a session — the latest chain head this machine saw and every
//! receipt not yet witnessed go to the hub first, the service key is pinned
//! or checked, and the receipts a checkpoint covers are taken off — then
//! describes the scopes this machine began before any hub did, uploads the
//! frames of its own streams past what the hub has, takes the frames of the
//! streams it holds past what it has, under the hub's signed epochs and
//! gaps, and checks where each stream stands, cutting back to where it
//! last agreed with the hub. A frame the hub refused takes its stream
//! back before it; what it held is committed again in a new epoch the hub
//! opens for this machine.
//!
//! Passes coalesce: a wake while one runs brings one more round. A pass
//! that fails tries again later; leaving the hub ends the driver.

use super::{Assets, Pinning};
use crate::Node;
use fastctx_content::v2::{EpochFact, Header, Holder, Imported, Library, Network, RawFrame, Receipt, Said};
use fastctx_net::link::Link;
use fastctx_net::sign::PublicKey;
use fastctx_types::access::HubId;
use fastctx_types::assets::{self as wire, Bytes, OLDEST, Readers, Reply, Request, StreamClass, checkpoint_statement};
use std::sync::Arc;
use std::time::Duration;

/// How often a driver looks again when nothing woke it: what changed on the
/// hub is pushed, what is committed here wakes it, so this only catches a
/// push lost on the way (design §28.1).
const AGAIN: Duration = Duration::from_secs(30);
/// How long the hub's answer to one request is waited for before the node
/// says it waits, and a pass with the hub goes on before the node says so.
const ANSWERED_WITHIN: Duration = Duration::from_secs(30);
const PASSED_WITHIN: Duration = Duration::from_secs(120);
/// How long one step of a pass goes on before the node says which step.
const STEP_WITHIN: Duration = Duration::from_secs(60);

/// A step of a pass with `hub`, told by the node watching itself as it goes
/// on past its time (`stuck`).
fn phase(hub: &HubId, step: &str) -> fastctx_platform::doing::OutsideWait {
    fastctx_platform::doing::waiting_outside(format!("{step}, in a pass with {hub}"), STEP_WITHIN)
}
/// How often every scope of a hub is looked at whole — every stream asked,
/// every cursor checked — when nothing called for it sooner (design §28.1).
const LOOK_WHOLE: Duration = Duration::from_secs(24 * 60 * 60);
/// Streams asked after in one page of what changed.
const CHANGES_AT_ONCE: u32 = 256;
/// The classes of streams, in the order they are caught up in after being
/// away: small state first (design §28.1).
const CLASSES: [u8; 4] = [0, 1, 2, 3];
/// Frames uploaded or taken at once.
const FRAMES_AT_ONCE: usize = 16;
/// How far along one stream a call takes it at most, in records (its
/// sequence numbers): a very long stream behind holds up no other for
/// longer, and goes on the next pass, which comes at once (H05). A pass may
/// come to one stream twice, from a page and again by its class (I07).
const RECORDS_A_TURN: u64 = 512;
/// How many frames of one stream go up in one pass: what is left goes at
/// once in the next, the other scopes gone through meanwhile — a hub put
/// back lacking a long stream of this machine's (its log) holds up no
/// other scope's pull while that stream goes up again.
const FRAMES_UP_A_TURN: usize = 256;
/// What a stream not yet caught up when its turn ended says: not a failure.
const GOES_ON: &str = "goes on next pass: ";
/// Receipts reported in one page.
const REPORTED_AT_ONCE: usize = 256;
/// How many times a frame's objects go before the hub must take it: put,
/// then proved where the put was not all of it.
const OBJECT_ROUNDS: usize = 3;

/// Scopes a pass goes through at once with a hub that keeps asset lines:
/// each of them on lines of its own, one that is long holding up no other.
const SCOPES_AT_ONCE: usize = 4;

/// What a device proposes a scope it begins be described as.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Proposal {
    /// The hub it is to be described by: one hub's scope goes to no other.
    pub(crate) hub: String,
    pub(crate) readers: Readers,
    pub(crate) classes: Vec<StreamClass>,
    /// When the account a conversation's scope was begun for was opened, in
    /// milliseconds: which account of its name it is, the scope described for
    /// that one alone. Kept here only; none where it was not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) born: Option<i64>,
}

/// What describing a scope ends with when its hub will never describe it
/// (`undescribed`): kept as such, not a failure.
const GONE: &str = "never to be described: ";

/// Brings the driver of `hub` round, starting it where none runs.
pub(crate) fn wake(node: &Arc<Node>, hub: &HubId) {
    let Some(assets) = node.assets.as_ref() else { return };
    let started = {
        let mut drivers = assets.drivers.lock().expect("drivers");
        if let Some(wake) = drivers.get(hub) {
            wake.notify_one();
            None
        } else {
            let wake = Arc::new(tokio::sync::Notify::new());
            drivers.insert(hub.clone(), wake.clone());
            Some(wake)
        }
    };
    if let Some(wake) = started {
        let (node, hub) = (Arc::downgrade(node), hub.clone());
        fastctx_platform::task::spawn(async move { run(node, hub, wake).await });
    }
}

/// The hub told this machine something unasked: a stream moved, or a fact
/// came, and its driver comes round; a checkpoint covering receipts of the
/// open session takes them off, under the pinned key.
pub(crate) fn pushed(node: &Arc<Node>, hub: &HubId, push: wire::Push) {
    match push {
        wire::Push::Moved { scope, stream, next } => moved(node, hub, scope, stream, next),
        wire::Push::Fact { .. } => wake(node, hub),
        wire::Push::Revoked { scope } => {
            let Some(assets) = node.assets.as_ref() else { return };
            assets.forget_listing(hub);
            let (library, log, device) = (assets.library.clone(), node.log.clone(), assets.device());
            let root = assets.viewer_root.clone();
            // What this machine keeps of a conversation (`direct`) was never
            // on its way to the hub: none of it is unsent, and it ends with
            // the holding.
            let stays = super::standing::kept_here_of(node.link_of(hub).as_deref());
            let (weak, hub) = (Arc::downgrade(node), hub.clone());
            fastctx_platform::task::spawn(async move {
                let key = scope.clone();
                let ended = blocking(&library, move |library| {
                    let scope = key.clone();
                    library.revoke_keeping_but(&key, &device, move |stream| stays(&scope, stream))
                })
                .await;
                if let Err(error) = ended {
                    log.line(format_args!("ending what this machine held of {scope}: {error}"));
                }
                if let Err(error) = drop_viewed_page(root, scope.clone()).await {
                    log.line(format_args!("forgetting a cached timeline page: {error}"));
                }
                // Forgotten again once the holding ended: a listing begun
                // while it still stood skipped a scope it may hold again
                // (DD01). What the period held is purged in the upkeep.
                if let Some(node) = weak.upgrade() {
                    if let Some(assets) = node.assets.as_ref() {
                        assets.forget_listing(&hub);
                    }
                    super::upkeep::wake(&node);
                }
            });
        }
        wire::Push::Checkpointed { checkpoint } => {
            let Some(assets) = node.assets.as_ref() else { return };
            let Some((session, key)) = assets.current.lock().expect("sessions").get(hub).copied() else { return };
            let Some(head) = checkpoint.head.array() else { return };
            let statement =
                checkpoint_statement(&checkpoint.scope, checkpoint.change, checkpoint.parked, checkpoint.seq, &head);
            // Only a checkpoint the hub's word binds to a witness that holds
            // it lets go of the receipts it covers: one written as the hub
            // sealed, its witness frozen before it, does not (AM03).
            let witnessed = fastctx_net::sign::heard_word(&key, &statement, &checkpoint.word.0)
                .is_some_and(|(seq, carried)| seq > checkpoint.seq || (seq == checkpoint.seq && carried == head));
            if !witnessed || !assets.heard(hub, &key, &statement, &checkpoint.word.0) {
                return;
            }
            let (library, log) = (assets.library.clone(), node.log.clone());
            fastctx_platform::task::spawn(async move {
                let scope = checkpoint.scope.clone();
                if let Err(error) = blocking(&library, move |library| {
                    library.checkpointed(&checkpoint.scope, checkpoint.change, checkpoint.parked, session)
                })
                .await
                {
                    log.line(format_args!("taking in a checkpoint of {scope}: {error}"));
                }
            });
        }
    }
}

/// A stream of a scope moved on the hub: with a hub that keeps asset
/// lines, a replica's frames past what is here are taken at once, by
/// themselves, as a pass catching up what changed would take them
/// (`catch_up`) — a pass under way with other scopes holds them up no
/// longer. The driver comes round all the same, for all a pass sees to
/// besides: where this machine stands in the owner's library, and whatever
/// could not be taken here.
fn moved(node: &Arc<Node>, hub: &HubId, scope: String, stream: String, next: u64) {
    let Some(assets) = node.assets.as_ref() else { return };
    assets.lively(&scope);
    let session = assets.current.lock().expect("sessions").get(hub).copied();
    let (Some(link), Some((_, key))) = (node.link_of(hub).filter(|link| link.keeps_asset_lines()), session) else {
        return wake(node, hub);
    };
    // Where the machine that keeps a stream says it stands (`direct`): its
    // scope is gone through until the stream is there, though it cannot be
    // taken just now.
    if super::direct::stays(&link, &scope, &stream) {
        assets.direct.heard(&scope, &stream, next);
    }
    let (node, hub) = (node.clone(), hub.clone());
    fastctx_platform::task::spawn(async move {
        if let Some(assets) = node.assets.as_ref()
            && shared_elsewhere(&node, &hub).is_none()
            && !assets.copies.paused_now().contains(&scope)
        {
            let left = match taken_at_once(assets, &hub, &link, &key, &scope, &stream, next).await {
                // Left to the pass, which says why should it fail there too.
                Err(why) => !why.starts_with(REVOKED),
                // Taken as far as its machine stood when it was asked, a
                // stream said meanwhile to stand further is the pass's to
                // take (`direct`): no turn of the scope may be looking.
                Ok(()) => owes(assets, &scope).await,
            };
            if left {
                assets.relook.lock().expect("relook").insert(scope.clone());
            }
        }
        wake(&node, &hub);
    });
}

/// Takes the frames of the replica `stream` of `scope` up to `next`, where
/// the scope is held and pulled whole here and no turn of it is under way.
async fn taken_at_once(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, stream: &str, next: u64) -> Result<(), String> {
    let library = &assets.library;
    let turning = assets.turn_of(scope);
    // Gone through now, it is brought there to where the hub stands; a
    // stream its machine keeps (`direct`) is brought no further than that
    // machine said as the going through asked it, and is left to the pass.
    let Ok(_turn) = turning.try_lock() else {
        return if super::direct::stays(link, scope, stream) { Err("left to the pass".into()) } else { Ok(()) };
    };
    let (key_text, stream_text, device) = (scope.to_string(), stream.to_string(), assets.device());
    let held = blocking(library, move |library| {
        let Some(holding) = library.holding(&key_text)? else { return Ok(None) };
        // Not pulled whole yet, a scope is pulled by itself (I01); a stream
        // this machine orders goes up, never down; one whose epochs are not
        // all here has them taken by the pass first.
        if !library.is_pulled(&key_text)? || library.orderer(&key_text, &stream_text)? == Some(device) || library.known_last_epoch(&key_text, &stream_text)?.is_none() {
            return Ok(None);
        }
        let at = library.streams(&key_text)?.into_iter().find(|(name, _)| *name == stream_text).map_or(0, |(_, next)| next);
        Ok(Some((holding, at)))
    })
    .await?;
    // Kept by the machine that orders it (`direct`), a stream is taken from
    // that machine: the hub was told only where it stands. What cannot be
    // taken so now is left to the pass.
    let stays = super::direct::stays(link, scope, stream);
    let Some((holding, at)) = held else {
        return if stays { Err("left to the pass".into()) } else { Ok(()) };
    };
    let source = super::direct::source(assets, hub, link, scope, stream).await;
    if stays && source.is_none() {
        return Err("left to the pass".into());
    }
    pull_stream(assets, hub, link, key, scope, stream, holding, at, next, None, source.as_ref()).await
}

/// This machine committed records of its own to `scope`: with a hub that
/// keeps asset lines they go up at once, by themselves, a pass under way
/// with other scopes holding them up no longer. The driver comes round all
/// the same.
pub(crate) fn committed(node: &Arc<Node>, hub: &HubId, scope: &str) {
    let Some(assets) = node.assets.as_ref() else { return };
    assets.lively(scope);
    let session = assets.current.lock().expect("sessions").get(hub).copied();
    let (Some(link), Some((session, key))) = (node.link_of(hub).filter(|link| link.keeps_asset_lines()), session) else {
        return wake(node, hub);
    };
    let (node, hub, scope) = (node.clone(), hub.clone(), scope.to_string());
    fastctx_platform::task::spawn(async move {
        if let Some(assets) = node.assets.as_ref() {
            let turning = assets.turn_of(&scope);
            // A few scopes at once: one more than that is left to the pass.
            if let Ok(_room) = assets.quick.clone().try_acquire_owned()
                && let Ok(_turn) = turning.try_lock()
            {
                // What does not go up now is left to the pass, which says why.
                let _ = upload(assets, &hub, &link, &key, session, &scope).await;
            }
        }
        wake(&node, &hub);
    });
}

async fn run(node: std::sync::Weak<Node>, hub: HubId, wake: Arc<tokio::sync::Notify>) {
    let _doing = fastctx_platform::doing::doing(format!("synchronizing assets with {hub}"));
    let mut again = crate::again::Again::default();
    let mut previous: Option<String> = None;
    loop {
        let Some(node) = node.upgrade() else { return };
        // A network left, or a place the hub ended, ends the driver; one
        // joined again starts it anew.
        let network = match node.link_of(&hub) {
            Some(link) => super::membership::of_link(&node, &hub, &link)
                .await
                .map(|network| network.map(|network| (link, network))),
            None => Ok(None),
        };
        let failed = match network {
            Ok(None) => {
                if let Some(assets) = node.assets.as_ref() {
                    assets.drivers.lock().expect("drivers").remove(&hub);
                }
                return;
            }
            Ok(Some((link, network))) if link.view().asset_sync >= OLDEST => {
                // A pass that goes on past its time is told (`stuck`), with
                // what in it waits.
                let _passing = fastctx_platform::doing::waiting_outside(format!("a pass with {hub}"), PASSED_WITHIN);
                pass(&node, &hub, &link, &network).await.err()
            }
            Ok(Some(_)) => None,
            Err(why) => Some(format!("joining the network's assets here: {why}")),
        };
        // A stream whose turn ended before it was caught up goes on at once,
        // and that is no failure (H05).
        let more = node.assets.as_ref().is_some_and(|assets| assets.more.lock().expect("more").remove(&hub));
        let failed = failed.filter(|error| !(more && error.contains(GOES_ON)));
        let wait = match &failed {
            Some(error) => {
                if previous.as_ref() != Some(error) {
                    node.log.line(format_args!("synchronizing assets with {hub}: {error}; trying again"));
                    previous = Some(error.clone());
                }
                again.next()
            }
            None => {
                again = crate::again::Again::default();
                previous = None;
                if more { Duration::ZERO } else { AGAIN }
            }
        };
        let mut ways = node.ways.subscribe();
        drop(node);
        tokio::select! {
            () = tokio::time::sleep(wait) => {},
            () = wake.notified() => {},
            _ = ways.changed() => {},
        }
    }
}

/// What a scope's pass ends with when the hub says this machine reads the
/// scope no more: not a failure.
pub(super) const REVOKED: &str = "no longer read here: ";

/// What taking a stream's frames ends with when its scope was paused here
/// meanwhile (`copies`): not a failure, nor taken whole.
pub(super) const PAUSED: &str = "taken in here no more: ";

/// What a pull says of a stream its machine keeps that the machine gave
/// nothing of just now: taken from the hub as far as the hub holds it, it
/// is not taken for pulled whole, and fails nothing.
const UNREACHED_NOW: &str = "its machine gave nothing now: ";

/// Asks the hub `request` of a scope; where the hub says this machine reads
/// the scope no more, its holding here ends (design §4.5, §6.3).
pub(super) async fn ask(assets: &Assets, link: &Arc<Link>, request: Request) -> Result<Reply, String> {
    // Waited for past its time, the node says which (`stuck`).
    let _waiting = fastctx_platform::doing::waiting_outside(format!("the hub's answer to {}", request.name()), ANSWERED_WITHIN);
    let reply = link.assets(request).await?;
    heard(assets, link, reply).await
}

/// What the hub answered of a scope, however it was asked: where it says
/// this machine reads the scope no more, its holding here ends.
pub(super) async fn heard(assets: &Assets, link: &Arc<Link>, reply: Reply) -> Result<Reply, String> {
    match reply {
        Reply::Revoked { scope } => {
            // What may be given back after it is found by listing all again:
            // its description is older than any listing goes on from (BB13).
            assets.forget_listing(&link.membership().id);
            let key = scope.clone();
            let device = assets.device();
            // As where the hub pushed it (`pushed`): nothing of a stream
            // kept here is unsent.
            let stays = super::standing::kept_here_of(Some(link.as_ref()));
            blocking(&assets.library, move |library| {
                let scope = key.clone();
                library.revoke_keeping_but(&key, &device, move |stream| stays(&scope, stream))
            })
            .await?;
            // And again once the holding ended (DD01).
            assets.forget_listing(&link.membership().id);
            if let Err(error) = drop_viewed_page(assets.viewer_root.clone(), scope.clone()).await {
                return Err(format!("{REVOKED}{scope}; forgetting a cached timeline page: {error}"));
            }
            Err(format!("{REVOKED}{scope}"))
        }
        reply => Ok(reply),
    }
}

async fn drop_viewed_page(root: std::path::PathBuf, scope: String) -> std::io::Result<()> {
    fastctx_platform::task::blocking(move || fastctx_client::cache::Cache::under(&root)?.forget_last_page_scope(&scope)).await
}

/// Runs `work` on the library off the async threads.
pub(super) async fn blocking<T: Send + 'static>(
    library: &Library,
    work: impl FnOnce(&Library) -> fastctx_content::Result<T> + Send + 'static,
) -> Result<T, String> {
    let library = library.clone();
    // Waited for past its time, a lock held elsewhere say, the node says so.
    let _waiting = fastctx_platform::doing::waiting_outside("work on the asset library", ANSWERED_WITHIN);
    fastctx_platform::task::spawn_blocking(move || work(&library))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

fn array(bytes: &Bytes, what: &str) -> Result<[u8; 32], String> {
    bytes.array().ok_or_else(|| format!("{what} is 32 bytes"))
}

/// One pass with the hub, on its network under `network`'s term: a scope
/// installed after the machine left it is not.
async fn pass(node: &Arc<Node>, hub: &HubId, link: &Arc<Link>, network: &Network) -> Result<(), String> {
    let assets = node.assets.as_ref().ok_or("the asset library is unavailable")?;
    let library = &assets.library;
    let session = assets.session();
    // The revision this machine's waiting shares wait at as the pass
    // begins: only a pass that went through every scope at it may say it
    // cleared at it.
    let waited = waiting_revision(link);
    let key = {
        let _phase = phase(hub, "opening the session");
        open(assets, hub, link, session).await?
    };
    let mut failed = {
        let _phase = phase(hub, "describing the scopes made here");
        describe(node, assets, hub, link, &key, network).await.err()
    };
    // What wanted a stream taken over as its descriptors came is made again
    // (MM14).
    let taken: Vec<String> = std::mem::take(&mut *assets.taken_over.lock().expect("taken over"));
    for scope in taken {
        if let Ok(resident) = fastctx_types::ResidentId::try_from(scope.rsplit('/').next().unwrap_or_default()) {
            crate::timeline::described(node, &resident);
        }
    }
    // Shared whole on another network, this machine takes nothing of this
    // one: what it begins goes up, nothing comes down to its disk.
    let isolated = shared_elsewhere(node, hub);
    let mut listed = None;
    if isolated.is_none() {
        let _phase = phase(hub, "listing what this machine may hold");
        match discover(node, assets, hub, link, &key, network).await {
            Ok(whole) => listed = whole,
            Err(why) => {
                failed.get_or_insert(format!("listing what this machine may hold: {why}"));
            }
        }
    }
    // This hub's scopes alone: another hub's go to no other.
    let theirs = hub.as_str().to_string();
    let held = blocking(library, move |library| library.live_scopes_of(&theirs)).await?;
    // What this machine takes nothing of in now (`copies`), settled again:
    // its turn sends up and gives back as ever, and takes nothing down; it
    // is not pulled by itself for being new, nor caught up by its changes.
    // Looked at whole, or found missing from a whole listing, it is gone
    // through all the same: whether it may be held still is asked as of any.
    let paused = {
        let settling = node.clone();
        match fastctx_platform::task::spawn_blocking(move || super::copies::settle(&settling, false)).await {
            Ok(Ok(paused)) => paused,
            Ok(Err(why)) => {
                node.log.line(format_args!("assets with {hub}: what is taken in here no more: {why}"));
                assets.copies.paused_now()
            }
            Err(_) => assets.copies.paused_now(),
        }
    };
    // Not pulled whole in its holding period yet — installed, held anew,
    // described otherwise, its pull cut short, or taken in again after it
    // was paused — a scope is looked at by itself until it is: no change
    // number after this machine's need show its streams' past (design
    // §28.1, E01). Read once what is paused is settled, so one taken in
    // again just now is pulled by itself this very pass.
    let theirs = hub.as_str().to_string();
    let unpulled = blocking(library, move |library| library.unpulled_of(&theirs)).await?;
    // Every scope is looked at whole when the driver begins, a day after
    // it last was, after the hub was put back, and while a share of this
    // machine waits for it to clear; otherwise only those that call for it
    // are, and what changed on the hub is asked after (design §28.1).
    let whole =
        waited.is_some() || assets.looked.lock().expect("looked").get(hub).is_none_or(|at| at.elapsed() >= LOOK_WHOLE);
    let pending = uploads_of(assets, hub).await?;
    let taken_over: std::collections::BTreeSet<String> =
        assets.takeovers.lock().expect("takeovers").iter().map(|(scope, _)| scope.clone()).collect();
    let stood: std::collections::BTreeSet<String> = {
        let theirs = hub.as_str().to_string();
        blocking(library, move |library| library.feed_owners(&theirs)).await?.into_iter().collect()
    };
    let mut owners = stood.clone();
    let mut scopes = Vec::new();
    // Each scope's owner library: one held here with no place in it kept yet
    // is asked where it stands every pass until it is (II01).
    let mut owner_of: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (scope, descriptor, holding) in held {
        let fresh = unpulled.contains(&scope) && !paused.contains(&scope);
        if let Ok(read) = serde_json::from_slice::<OwnerOf>(&descriptor) {
            if whole || fresh || !stood.contains(&read.owner) {
                owners.insert(read.owner.clone());
            }
            owner_of.insert(scope.clone(), read.owner);
        }
        // Held, yet not among all a whole listing says this machine may
        // hold: asked after by itself, the hub says whether it may hold it
        // still — access taken away shows so, and nowhere else (§28.1).
        let unlisted = listed.as_ref().is_some_and(|listed: &std::collections::BTreeSet<String>| !listed.contains(&scope));
        let again = assets.relook.lock().expect("relook").contains(&scope);
        if whole || fresh || unlisted || again || pending.contains(&scope) || taken_over.contains(&scope) {
            scopes.push((scope, holding, fresh));
        }
    }
    // Where each owner library stands as a whole look begins, or as the
    // first scopes of one are looked at: what changes after it is asked
    // after from then on.
    let fresh: std::collections::BTreeSet<String> =
        if whole { owners.clone() } else { owners.difference(&stood).cloned().collect() };
    let marks = if isolated.is_none() && !fresh.is_empty() {
        let _phase = phase(hub, "asking where the owner libraries stand");
        tops(assets, link, &fresh).await.ok()
    } else {
        None
    };
    // Not told where they stand, nothing of this look counts as done: no
    // scope is marked pulled whole and no whole look is taken as made, so
    // the next pass asks again (HH02).
    let untold = isolated.is_none() && !fresh.is_empty() && marks.is_none();
    if untold {
        failed.get_or_insert("where the hub's owner libraries stand could not be asked".to_string());
    }
    // The owner libraries some scope of which this pass did not go through
    // whole: where they stand is not taken as reached (II01).
    let mut short: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // A scope that fails holds up none beside it; the pass reports one
    // failure. Those that moved or were committed to lately go first; with
    // a hub that keeps asset lines several go at once.
    let lively = assets.lively_at();
    scopes.sort_by(|one, other| lively.get(&other.0).cmp(&lively.get(&one.0)).then_with(|| one.0.cmp(&other.0)));
    let room = Arc::new(tokio::sync::Semaphore::new(if link.keeps_asset_lines() { SCOPES_AT_ONCE } else { 1 }));
    let alone = isolated.is_some();
    let mut turning = Vec::with_capacity(scopes.len());
    for (scope, holding, first) in scopes {
        let (node, hub, link, room) = (node.clone(), hub.clone(), link.clone(), room.clone());
        turning.push(fastctx_platform::task::spawn(async move {
            let _room = room.acquire_owned().await;
            // Paused while it waited its turn, it takes nothing down either.
            let alone = alone || node.assets.as_ref().is_some_and(|assets| assets.copies.paused_now().contains(&scope));
            turn(&node, &hub, &link, &key, session, scope, holding, first, alone, untold).await
        }));
    }
    for turned in turning {
        let Ok(turned) = turned.await else {
            failed.get_or_insert("a scope's turn ended before it was through".to_string());
            continue;
        };
        if let Some(why) = turned.failed {
            failed.get_or_insert(why);
        }
        // Not gone through whole, its owner library is not taken as reached.
        if turned.short
            && let Some(owner) = owner_of.get(&turned.scope)
        {
            short.insert(owner.clone());
        }
    }
    if whole && !untold {
        assets.looked.lock().expect("looked").insert(hub.clone(), std::time::Instant::now());
    }
    // Each library every scope of which was gone through whole stands
    // where it stood as the look began (II01).
    for (owner, generation, top, at) in marks.unwrap_or_default() {
        if short.contains(&owner) {
            continue;
        }
        let theirs = hub.as_str().to_string();
        let mark = owner.clone();
        blocking(library, move |library| library.fed(&theirs, &owner, &generation, top, at)).await?;
        super::standing::stood(assets, hub, &mark);
    }
    if !whole && isolated.is_none() {
        for owner in &stood {
            // Which scopes are not pulled whole yet is read again here: one
            // pulled by itself earlier in this pass is caught up by its
            // changes from now on, a change after that pull included — read
            // as the pass began, it would be passed over for good.
            let theirs = hub.as_str().to_string();
            let unpulled = blocking(library, move |library| library.unpulled_of(&theirs)).await?;
            let _phase = phase(hub, &format!("taking what changed in {owner}"));
            if let Err(why) = feed(assets, hub, link, &key, owner, &unpulled).await {
                failed.get_or_insert(format!("what changed in {owner}: {why}"));
            }
        }
    }
    // What ended holding periods left goes a pass at a time.
    let purging = phase(hub, "purging what ended");
    if let Err(why) = blocking(library, |library| library.purge()).await {
        failed.get_or_insert(format!("purging what ended: {why}"));
    }
    drop(purging);
    if failed.is_none()
        && whole
        && let Some(revision) = waited
        && waiting_revision(link) == Some(revision)
        && let Err(why) = {
            let _step = phase(hub, "clearing what this machine no longer holds");
            cleared(node, assets, hub, link, revision).await
        }
    {
        failed = Some(why);
    }
    if let Some(other) = isolated {
        node.log.line(format_args!(
            "assets with {hub}: this machine is shared whole on {other}; nothing of {hub}'s comes to it"
        ));
        // Where it stands goes to the hub all the same (KK05).
        if let Err(why) = super::standing::report(assets, hub, link).await {
            node.log.line(format_args!("assets with {hub}: telling where this machine stands: {why}"));
        }
        return failed.map_or(Ok(()), Err);
    }
    // The machines that order what this one holds of conversations are asked
    // where their streams stand, now and then (`direct`): a scope left
    // behind by a word of moving lost on the way is gone through at once.
    if assets.direct.look_due(hub) {
        let _step = phase(hub, "asking the machines that keep conversations where they stand");
        match super::direct::look(assets, hub, link).await {
            Ok(behind) if !behind.is_empty() => {
                assets.relook.lock().expect("relook").extend(behind);
                assets.more.lock().expect("more").insert(hub.clone());
            }
            Ok(_) => {}
            Err(why) => node.log.line(format_args!("assets with {hub}: asking where kept conversations stand: {why}")),
        }
    }
    // With a hub that keeps asset lines, objects are fetched off the passes
    // (`objects::lane`): no pass waits for one.
    if link.keeps_asset_lines() {
        super::objects::lane(node, hub);
    } else {
        let _step = phase(hub, "fetching objects");
        for failed in super::objects::fetch(assets, hub, link).await {
            node.log.line(format_args!("assets with {hub}: {failed}; asked again later"));
        }
    }
    // Where this machine stands goes to the hub as it changes (design §28.9).
    let _step = phase(hub, "telling where this machine stands");
    if let Err(why) = super::standing::report(assets, hub, link).await {
        node.log.line(format_args!("assets with {hub}: telling where this machine stands: {why}"));
    }
    // What it holds of the conversation streams it orders is vouched for,
    // beside the passes: the hub may let go of its own copies (`vouch`).
    super::vouch::start(node, hub, link);
    failed.map_or(Ok(()), Err)
}

/// Whether a stream of `scope` kept by its machine (`direct`) was lately
/// said to stand further there than it stands here.
async fn owes(assets: &Assets, scope: &str) -> bool {
    let key = scope.to_string();
    match blocking(&assets.library, move |library| library.streams(&key)).await {
        Ok(here) => assets.direct.owed(scope, &here.into_iter().collect()),
        Err(_) => false,
    }
}

/// What a scope's turn in a pass came to.
struct Turned {
    scope: String,
    /// The first of its steps that failed, and why.
    failed: Option<String>,
    /// Whether its owner library is not to be taken as reached: it was not
    /// gone through whole.
    short: bool,
}

/// One scope's turn in a pass: its own streams go up, the hub's come down,
/// where each stands is checked, and the streams to be ordered here are
/// taken over; `first` when it was not pulled whole in its holding period
/// yet, `isolated` when nothing of the hub's comes down to this disk.
#[allow(clippy::too_many_arguments)]
async fn turn(
    node: &Arc<Node>,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    session: u64,
    scope: String,
    holding: i64,
    first: bool,
    isolated: bool,
    untold: bool,
) -> Turned {
    let Some(assets) = node.assets.as_ref() else {
        return Turned { scope, failed: Some("the asset library is unavailable".into()), short: true };
    };
    let mut failed: Option<String> = None;
    let turning = assets.turn_of(&scope);
    let _turn = turning.lock().await;
    let scope = &scope;
    // Where it stood in being taken in again as this turn began: taken in
    // again meanwhile, it is not marked pulled whole by this turn.
    let released_at = assets.copies.release_mark(scope);
    let _phase = phase(hub, &format!("going through {scope}"));
    // Each step goes on whatever the one before met; a revoked scope goes
    // no further.
    let step = phase(hub, &format!("going up {scope}"));
    let uploaded = upload(assets, hub, link, key, session, scope).await;
    drop(step);
    if uploaded.is_ok() {
        assets.uploads.lock().expect("uploads").remove(scope);
    }
    let step = phase(hub, &format!("coming down {scope}"));
    let pulled = match &uploaded {
        Err(why) if why.starts_with(REVOKED) => Err(why.clone()),
        _ => pull(assets, hub, link, key, session, scope, holding, isolated).await,
    };
    drop(step);
    // What the hub lacks of this machine's own streams, found as they were
    // pulled — the hub put back (§17) — goes up again this very pass, from
    // where the hub is.
    let uploaded = if uploaded.is_ok() && assets.uploads.lock().expect("uploads").contains(scope) {
        let _step = phase(hub, &format!("going up again {scope}"));
        let again = upload(assets, hub, link, key, session, scope).await;
        if again.is_ok() {
            assets.uploads.lock().expect("uploads").remove(scope);
        }
        again
    } else {
        uploaded
    };
    // Isolated, what the hub lacks of what is here still goes back to it
    // (AA04); nothing comes down.
    if isolated {
        let pulled = pulled.and_then(|(_, failed)| failed.map_or(Ok(()), Err));
        let mut whole_here = true;
        for done in [uploaded, pulled] {
            if let Err(why) = done
                && !why.starts_with(REVOKED)
            {
                whole_here = false;
                failed.get_or_insert(format!("{scope}: {why}"));
            }
        }
        // What the hub still lacks goes back next pass too (II02).
        let mut relook = assets.relook.lock().expect("relook");
        if whole_here {
            relook.remove(scope);
        } else {
            relook.insert(scope.clone());
        }
        return Turned { scope: scope.clone(), failed, short: false };
    }
    // Where each stream stands is checked whatever a stream's frames met: a
    // frame that does not chain to what is here is what checking finds the
    // shared place for.
    let step = phase(hub, &format!("checking where {scope} stands"));
    let checked = match &pulled {
        Err(why) if why.starts_with(REVOKED) => Err(why.clone()),
        Err(_) => Ok(false),
        Ok((ahead, _)) => cursors(assets, link, scope, holding, ahead).await,
    };
    drop(step);
    // What checking cut back comes again from the hub this very pass (HH05).
    let (pulled, checked) = match checked {
        Ok(true) => (pull(assets, hub, link, key, session, scope, holding, false).await, Ok(())),
        Ok(false) => (pulled, Ok(())),
        Err(why) => (pulled, Err(why)),
    };
    // Holding what the hub has of them, the streams to be ordered here are
    // taken over (design §8).
    let step = phase(hub, &format!("taking over the streams of {scope}"));
    // A machine away just now holds up no takeover: what is to be ordered
    // here may be its very streams.
    let checked = match (&pulled, checked) {
        (Ok((_, None)), Ok(())) => super::takeover::run(node, assets, hub, link, key, scope).await,
        (Ok((_, Some(why))), Ok(())) if why.starts_with(UNREACHED_NOW) => {
            super::takeover::run(node, assets, hub, link, key, scope).await
        }
        (_, checked) => checked,
    };
    drop(step);
    // Every stream pulled and checked: the scope is pulled whole in its
    // holding period, and the change numbers carry it from now on.
    // Paused meanwhile, it is not taken for pulled whole: what passes it by
    // from now on is owed (`copies`).
    if first
        && !untold
        && matches!(&pulled, Ok((_, None)))
        && checked.is_ok()
        && !assets.copies.paused_now().contains(scope)
    {
        // Taken in again since the turn began, it is marked by a later one.
        let (marking, key_text) = (node.clone(), scope.clone());
        let marked = fastctx_platform::task::blocking(move || {
            let assets = marking.assets.as_ref().ok_or("the asset library is unavailable")?;
            assets.copies.mark_pulled(&assets.library, &key_text, holding, released_at)
        })
        .await;
        match marked {
            // Pulled whole, its copy is read here from now: what the
            // interface lists of conversations is made again (plan 3-5).
            Ok(true) => assets.metadata_moved.send_modify(|count| *count = count.wrapping_add(1)),
            Ok(false) => {}
            Err(why) => {
                failed.get_or_insert(why);
            }
        }
    }
    let pulled = pulled.and_then(|(_, failed)| failed.map_or(Ok(()), Err));
    let mut whole_here = true;
    for done in [uploaded, pulled, checked] {
        if let Err(why) = done
            && !why.starts_with(REVOKED)
        {
            whole_here = false;
            // Paused part way, or its machine away just now, it is gone
            // through again, and fails nothing.
            if !why.starts_with(PAUSED) && !why.starts_with(UNREACHED_NOW) {
                failed.get_or_insert(format!("{scope}: {why}"));
            }
        }
    }
    // A stream its machine keeps (`direct`), lately said to stand further
    // there than it does here, has its scope gone through again though
    // nothing failed: its machine did not give it just now.
    let owed = owes(assets, scope).await;
    // Not gone through whole, it is looked at by itself again next pass
    // until it is (HH03, HH06).
    {
        let mut relook = assets.relook.lock().expect("relook");
        if whole_here && !owed {
            relook.remove(scope);
        } else {
            relook.insert(scope.clone());
        }
    }
    // Said to stand further while that was decided, it is left for the next
    // pass all the same: what said so may have asked for it just before.
    if !owed && owes(assets, scope).await {
        assets.relook.lock().expect("relook").insert(scope.clone());
    }
    Turned { scope: scope.clone(), failed, short: !whole_here }
}

/// Another network this machine is shared whole on, a share waiting or in
/// force: one such network's holders may read the whole disk.
pub(crate) fn shared_elsewhere(node: &Node, hub: &HubId) -> Option<HubId> {
    node.links().iter().find_map(|link| {
        let other = link.membership().id;
        if other == *hub {
            return None;
        }
        let view = link.view();
        let me = view.directory.canonical(&link.name()).clone();
        view.directory.shares.iter().any(|share| share.machine == me).then_some(other)
    })
}

/// Shares of this machine waiting for it to clear (design §6.3): once a
/// whole pass found nothing it may no longer hold — each scope asked of
/// the hub as it judges with the waiting shares — and the records it may
/// no longer keep are pruned, it says so under its key, at the holding
/// revision the shares wait at.
async fn cleared(
    node: &Arc<Node>,
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    revision: u64,
) -> Result<(), String> {
    let me = link.name();
    let view = link.view();
    let me = view.directory.canonical(&me).clone();
    // What was said is the revision and who waited at it: a new share at a
    // revision said before is said again.
    let waiting: Vec<String> = view
        .directory
        .shares
        .iter()
        .filter(|share| share.machine == me && share.pending.is_some())
        .map(|share| share.account.to_string())
        .collect();
    let said = (revision, waiting);
    if assets.cleared.lock().expect("cleared").get(hub) == Some(&said) {
        return Ok(());
    }
    if !crate::replicas::cleanup::started(node) {
        // The records' pruning has not begun yet: said at a later pass.
        return Ok(());
    }
    // On another network too, or holding what one gave it, this machine
    // holds what that network's accounts may see: whole access through this
    // one waits.
    if let Some(why) = elsewhere(assets, hub).await? {
        node.log.line(format_args!("whole access to this machine through {hub} waits: {why}"));
        return Ok(());
    }
    // Nor while a reading is kept on disk to be placed (design §28.51): none
    // begins until this is said — neither its work store nor its objects put
    // as it is read — and before anything is purged, every one under way has
    // ended, its objects put and its leases let go; what one left behind is
    // taken away here, and while it cannot be, this waits.
    let _held_off = crate::timeline::spill::hold_off();
    if crate::timeline::spill::kept() > 0 {
        node.log.line(format_args!("whole access to this machine through {hub} waits: a reading is kept on disk"));
        return Ok(());
    }
    // Nor the histories cut for viewers from what it held (design §28.52
    // 52-1): read no more from now, each goes as its viewer is done; until
    // none is left, this waits.
    crate::cut_history::advance();
    node.cut_histories.clear();
    let world = node.world.path().to_path_buf();
    match fastctx_platform::task::spawn_blocking(move || crate::cut_history::cleared(&world)).await {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            node.log.line(format_args!("whole access to this machine through {hub} waits: a history cut for a viewer is still read"));
            return Ok(());
        }
        Ok(Err(error)) => {
            node.log.line(format_args!("whole access to this machine through {hub} waits: a history cut for a viewer is not taken away: {error}"));
            return Ok(());
        }
        Err(error) => {
            node.log.line(format_args!("whole access to this machine through {hub} waits: {error}"));
            return Ok(());
        }
    }
    let left = assets.spill_dir();
    if let Err(error) = fastctx_platform::task::blocking(move || crate::timeline::spill::clear(&left)).await {
        node.log.line(format_args!("whole access to this machine through {hub} waits: what a reading kept on disk left is not taken away: {error}"));
        return Ok(());
    }
    crate::replicas::publication::prune(node).await?;
    let root = node.root.clone();
    fastctx_platform::task::blocking(move || fastctx_client::cache::Cache::under(&root)?.clear_last_pages())
        .await.map_err(|error| format!("clearing cached timeline pages: {error}"))?;
    // No object fetched off the passes is put from here until this ends, and
    // none asked for before it is kept after it (`objects::asked`): what
    // was cleared does not come back behind the word that it is.
    let _clearing = assets.clearing.write().await;
    assets.cleared_times.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    // What ended is purged whole, and the objects only it held go now, not
    // after the grace: the new holder finds none of it on the disk.
    let library = &assets.library;
    for _ in 0..100_000 {
        if !blocking(library, |library| library.purging()).await? {
            break;
        }
        blocking(library, |library| library.purge()).await?;
    }
    if blocking(library, |library| library.purging()).await? {
        return Ok(());
    }
    // Every object nothing that stays refers to goes, whatever kept it
    // back: half-made transfers, a deletion that failed, the grace. One a
    // live lease alone keeps goes at a later pass.
    if let Some(left) = blocking(library, |library| library.clear_unrooted()).await? {
        node.log.line(format_args!("whole access to this machine through {hub} waits: {left}"));
        return Ok(());
    }
    // Nor while an original is written out to be read: it goes as its
    // reading ends (QQ18).
    if crate::attachments::kept_here::passing(node) {
        node.log.line(format_args!("whole access to this machine through {hub} waits: an attachment is being read"));
        return Ok(());
    }
    // Said only if, after all that, still nothing of another network's is
    // here and no other network came meanwhile.
    if elsewhere(assets, hub).await?.is_some() || waiting_revision(link) != Some(revision) {
        return Ok(());
    }
    // What this machine did and spent so far stays its accounts' own, never
    // the body's it is to be (§29, 2.5).
    let (marking, marked) = (node.clone(), hub.clone());
    fastctx_platform::task::blocking(move || super::activity::mark_private(&marking, &marked)).await?;
    let proof = assets.sign_cleared(me.as_str(), revision);
    let Reply::Cleared { activated } = link.assets(Request::Cleared { revision, proof: proof.into() }).await? else {
        return Err("the hub did not take this machine's word that it cleared".into());
    };
    assets.cleared.lock().expect("cleared").insert(hub.clone(), said);
    if activated > 0 {
        node.log.line(format_args!("cleared for {activated} share(s) of this machine; they are in force now"));
    }
    Ok(())
}

/// What of another hub's this machine has, if anything: a driver for it, a
/// scope it described still held here, or one this machine began for it.
async fn elsewhere(assets: &Assets, hub: &HubId) -> Result<Option<String>, String> {
    if assets.drivers.lock().expect("drivers").keys().any(|other| other != hub) {
        return Ok(Some("this machine is on another network as well".into()));
    }
    let library = &assets.library;
    for (scope, descriptor) in blocking(library, |library| library.described_scopes()).await? {
        let theirs = serde_json::from_slice::<wire::Descriptor>(&descriptor)
            .map_or(true, |descriptor| descriptor.hub != hub.as_str());
        let held = {
            let scope = scope.clone();
            blocking(library, move |library| library.holding(&scope)).await?.is_some()
        };
        if theirs && held {
            return Ok(Some(format!("it holds {scope}, another network's")));
        }
    }
    for (scope, proposal) in blocking(library, |library| library.provisionals()).await? {
        let theirs = proposal
            .as_deref()
            .and_then(|proposal| serde_json::from_slice::<Proposal>(proposal).ok())
            .is_none_or(|proposal| proposal.hub != hub.as_str());
        if theirs {
            return Ok(Some(format!("it began {scope} for another network")));
        }
    }
    Ok(None)
}

/// The scopes `hub` described here, held now or not.
pub(super) async fn hub_scopes(library: &Library, hub: &HubId) -> Result<std::collections::BTreeSet<String>, String> {
    let hub = hub.as_str().to_string();
    Ok(blocking(library, move |library| library.described_of(&hub)).await?.into_iter().collect())
}

/// The scopes of `hub` with something of this machine's own committed and
/// perhaps not uploaded yet: those the library says were committed to since
/// they were last taken, and those an upload of did not finish.
async fn uploads_of(assets: &Assets, hub: &HubId) -> Result<std::collections::BTreeSet<String>, String> {
    let library = &assets.library;
    // The first pass since the node started finds on the disk what was
    // committed and not uploaded before it stopped: a pass that fails then
    // leaves it to the next, not to the next whole look (HH03).
    if assets.seeded.lock().expect("seeded").insert(hub.clone()) {
        let (theirs, device) = (hub.as_str().to_string(), assets.device());
        let durable = blocking(library, move |library| {
            let mut durable = Vec::new();
            for (scope, _, _) in library.live_scopes_of(&theirs)? {
                if !library.uploads(&scope, &device)?.is_empty() {
                    durable.push(scope);
                }
            }
            Ok(durable)
        })
        .await;
        match durable {
            Ok(durable) => assets.uploads.lock().expect("uploads").extend(durable),
            Err(why) => {
                assets.seeded.lock().expect("seeded").remove(hub);
                return Err(why);
            }
        }
    }
    let committed = library.committed();
    let pending: Vec<String> = {
        let mut uploads = assets.uploads.lock().expect("uploads");
        uploads.extend(committed);
        uploads.iter().cloned().collect()
    };
    let mut theirs = std::collections::BTreeSet::new();
    for scope in pending {
        let key = scope.clone();
        match blocking(library, move |library| library.hub_of(&key)).await? {
            Some(of) if of == hub.as_str() => {
                theirs.insert(scope);
            }
            Some(_) => {}
            // Held no more: nothing of it goes up.
            None => {
                assets.uploads.lock().expect("uploads").remove(&scope);
            }
        }
    }
    Ok(theirs)
}

/// Where each of `owners`' libraries stands now: its generation, its latest
/// change and when that was.
async fn tops(
    assets: &Assets,
    link: &Arc<Link>,
    owners: &std::collections::BTreeSet<String>,
) -> Result<Vec<(String, String, u64, i64)>, String> {
    let mut marks = Vec::new();
    for owner in owners {
        let request = Request::Changes { owner: owner.clone(), after: 0, before: None, class: None, limit: 1 };
        let Reply::Changes { generation, top, top_at, .. } = ask(assets, link, request).await? else {
            return Err("the hub did not say where its library stands".into());
        };
        marks.push((owner.clone(), generation, top, top_at));
    }
    Ok(marks)
}

/// Catches up what changed in `owner`'s library of `hub` since this
/// machine last stood in it (design §28.1): newest first, a page at a time,
/// and by the classes of streams, small state first, when there is more
/// than a page; where it stands moves only once all of it is caught up.
async fn feed(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    owner: &str,
    unpulled: &std::collections::BTreeSet<String>,
) -> Result<(), String> {
    let library = &assets.library;
    let stood = {
        let (theirs, owner) = (hub.as_str().to_string(), owner.to_string());
        blocking(library, move |library| library.feed(&theirs, &owner)).await?
    };
    let page = |after: u64, before: Option<u64>, class: Option<u8>| {
        ask(assets, link, Request::Changes { owner: owner.to_string(), after, before, class, limit: CHANGES_AT_ONCE })
    };
    let mut after = stood.as_ref().map_or(0, |(_, change)| *change);
    let Reply::Changes { mut generation, mut streams, mut top, mut top_at, mut more, .. } =
        page(after, None, None).await?
    else {
        return Err("the hub did not say what changed".into());
    };
    // The hub was put back since: its numbers start again, and so does this;
    // what was taken in the generation before counts no more, from wherever
    // it stood (LL02, MM08).
    if stood.as_ref().is_some_and(|(was, _)| *was != generation) {
        super::standing::stood(assets, hub, owner);
        // Put back, the hub may hold a stream it orders shorter than this
        // replica does, which no change number shows: every scope is looked
        // at whole at once, where such a stream is kept aside and cut back
        // to the hub's (`pull`), though this machine saw nothing the hub
        // set aside.
        assets.looked.lock().expect("looked").remove(hub);
        assets.more.lock().expect("more").insert(hub.clone());
    }
    if stood.as_ref().is_some_and(|(was, _)| *was != generation) && after > 0 {
        after = 0;
        let Reply::Changes { generation: now, streams: all, top: at, top_at: when, more: further, .. } =
            page(0, None, None).await?
        else {
            return Err("the hub did not say what changed".into());
        };
        (generation, streams, top, top_at, more) = (now, all, at, when, further);
    }
    if generation.is_empty() || top <= after {
        if stood.is_none() && !generation.is_empty() {
            let (theirs, owner) = (hub.as_str().to_string(), owner.to_string());
            blocking(library, move |library| library.fed(&theirs, &owner, &generation, top, top_at)).await?;
        }
        return Ok(());
    }
    // What is taken counts toward this library, in this generation, while
    // this machine's listing is not forgotten (KK04).
    let era = assets.forgotten.load(std::sync::atomic::Ordering::SeqCst);
    let counted = super::standing::Counted { owner, generation: &generation, era };
    let mut failed = catch_up(assets, hub, link, key, &counted, unpulled, streams).await;
    // More than a page: class by class, each newest first.
    if more {
        for class in CLASSES {
            let mut before = Some(top + 1);
            loop {
                let Reply::Changes { streams, through, more, .. } = page(after, before, Some(class)).await? else {
                    return Err("the hub did not say what changed".into());
                };
                if let Some(why) = catch_up(assets, hub, link, key, &counted, unpulled, streams).await {
                    failed.get_or_insert(why);
                }
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
    let (theirs, owned) = (hub.as_str().to_string(), owner.to_string());
    blocking(library, move |library| library.fed(&theirs, &owned, &generation, top, top_at)).await?;
    super::standing::stood(assets, hub, owner);
    Ok(())
}

/// Brings each of `streams`, as the hub said they stand, to where they
/// stand here: a replica's frames taken; this machine's own streams go up
/// by their uploads instead. A scope not held here yet is listed again at
/// the next pass. The first failure, the rest gone on with. Of a scope this
/// machine takes nothing of in now (`copies`), a stream's descriptors are
/// taken, and none of its frames.
async fn catch_up(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    counted: &super::standing::Counted<'_>,
    unpulled: &std::collections::BTreeSet<String>,
    streams: Vec<wire::ChangedStream>,
) -> Option<String> {
    let library = &assets.library;
    let device = assets.device();
    let mut failed = None;
    for changed in streams {
        let done: Result<(), String> = async {
            // One goes through a scope at a time: what a push began taking
            // by itself is through before this looks where the stream is.
            let turning = assets.turn_of(&changed.scope);
            let _turn = turning.lock().await;
            let scope = changed.scope.clone();
            let Some(holding) = blocking(library, move |library| library.holding(&scope)).await? else {
                // Given to hold but not installed here yet (design §28.1).
                assets.forget_listing(hub);
                return Ok(());
            };
            // Not pulled whole in its holding period yet, it is pulled by
            // itself; its frames are not this library's to take here, nor
            // to count as taken (I01).
            if unpulled.contains(&changed.scope) {
                return Ok(());
            }
            // Handed over and not written in yet, the stream moved no head:
            // its new epoch is taken all the same, this machine's own
            // stream handed away included (I09).
            let (scope, stream) = (changed.scope.clone(), changed.stream.clone());
            let known = blocking(library, move |library| library.known_last_epoch(&scope, &stream)).await?;
            // A stream first heard of in a scope held here may stand for
            // another the hub gives this machine no more — a reader's
            // project ledger under the machines it may use now (§29, 2.4):
            // the scope is looked at whole at the next pass, the hub's
            // listing of what it withholds with it.
            if known.is_none() {
                assets.relook.lock().expect("relook").insert(changed.scope.clone());
            }
            if changed.epoch > 0 && known.is_none_or(|known| known < changed.epoch) {
                succession(assets, hub, link, key, &changed.scope, &changed.stream).await?;
            }
            if assets.copies.paused_now().contains(&changed.scope) {
                return Ok(());
            }
            let (scope, stream) = (changed.scope.clone(), changed.stream.clone());
            if blocking(library, move |library| library.orderer(&scope, &stream)).await? == Some(device) {
                return Ok(());
            }
            let scope = changed.scope.clone();
            let at = blocking(library, move |library| library.streams(&scope))
                .await?
                .into_iter()
                .find(|(name, _)| *name == changed.stream)
                .map_or(0, |(_, next)| next);
            pull_stream(assets, hub, link, key, &changed.scope, &changed.stream, holding, at, changed.next, Some(counted), None).await
        }
        .await;
        // Paused part way, it owes a whole pull already (`copies`): passed
        // over, as one paused before it was begun.
        if let Err(why) = done
            && !why.starts_with(REVOKED)
            && !why.starts_with(PAUSED)
        {
            failed.get_or_insert(format!("{} of {}: {why}", changed.stream, changed.scope));
        }
    }
    failed
}

/// The holding revision this machine's shares wait at, by the hub's view.
fn waiting_revision(link: &Link) -> Option<u64> {
    let view = link.view();
    let me = link.name();
    let me = view.directory.canonical(&me).clone();
    view.directory.shares.iter().filter(|share| share.machine == me).filter_map(|share| share.pending).max()
}

/// Opens a session: what this machine saw of the hub's history goes first.
async fn open(assets: &Assets, hub: &HubId, link: &Arc<Link>, session: u64) -> Result<PublicKey, String> {
    let library = &assets.library;
    let mine = hub_scopes(library, hub).await?;
    let mut reported = Vec::new();
    let mut after = None;
    loop {
        let mark = after.clone();
        let page = blocking(library, move |library| library.unwitnessed(mark.as_ref(), REPORTED_AT_ONCE)).await?;
        // Another hub's receipts go to no other.
        reported.extend(page.receipts.into_iter().filter(|(scope, _)| mine.contains(scope)));
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    let wired = reported
        .iter()
        .filter_map(|(scope, receipt)| {
            let said = match &receipt.said {
                Said::Applied { change, prefix } => wire::Said::Applied { change: *change, prefix: (*prefix).into() },
                Said::Parked { sequence, accumulated } => {
                    wire::Said::Parked { sequence: *sequence, accumulated: (*accumulated).into() }
                }
                Said::Rejected { .. } => return None,
            };
            Some(wire::Reported {
                scope: scope.clone(),
                stream: receipt.stream.clone(),
                epoch: receipt.epoch,
                first: receipt.first,
                digest: receipt.digest.into(),
                said,
                word: receipt.signature.clone().into(),
            })
        })
        .collect();
    let request = Request::Open {
        version: link.asset_version(),
        seen: assets.seen(hub),
        reported: wired,
        pinned: assets.pinned(hub),
        lineage: assets.lineage(hub),
    };
    let (offered, checkpointed, rotations, closed) = match link.assets(request).await? {
        Reply::Ready { key, clock, checkpointed, rotations, closed, .. } => {
            // What the hub's word that a device holds a scope is timed by
            // (`direct`).
            assets.direct.hub_clock(hub, clock);
            (key, checkpointed, rotations, closed)
        }
        Reply::Sealed { why, recovery } => {
            // Sealed for its recovery: what this machine holds of what the
            // hub signed past where it was put back is handed back (§28.38).
            if let Some(recovery) = recovery
                && let Err(error) = super::hand_back::hand_back(assets, hub, link, recovery).await
            {
                return Err(format!("the hub keeps every asset sealed: {why}; handing back what it lost stopped: {error}"));
            }
            return Err(format!("the hub keeps every asset sealed: {why}"));
        }
        _ => return Err("the hub did not open the session".into()),
    };
    let key = assets.pin_following(hub, &offered, &rotations)?;
    if closed {
        // The hub set the history this machine saw aside when it recovered:
        // its head is forgotten and its receipts wait no more; what the hub
        // lacks is given back as each scope is gone through (`pull`), in a
        // whole look this very pass.
        assets.looked.lock().expect("looked").remove(hub);
        assets.direct.put_back(hub);
        assets.forget_seen(hub).map_err(|error| error.to_string())?;
        let scopes: std::collections::BTreeSet<String> = reported.iter().map(|(scope, _)| scope.clone()).collect();
        for scope in scopes {
            blocking(library, move |library| library.void_receipts(&scope)).await?;
        }
    } else {
        blocking(library, move |library| library.vouched(session, reported)).await?;
    }
    assets.current.lock().expect("sessions").insert(hub.clone(), (session, key));
    for checkpoint in checkpointed {
        let head = array(&checkpoint.head, "a checkpoint's head")?;
        let statement =
            checkpoint_statement(&checkpoint.scope, checkpoint.change, checkpoint.parked, checkpoint.seq, &head);
        if !assets.heard(hub, &key, &statement, &checkpoint.word.0) {
            continue;
        }
        blocking(library, move |library| {
            library.checkpointed(&checkpoint.scope, checkpoint.change, checkpoint.parked, session)
        })
        .await?;
    }
    Ok(key)
}

/// The scopes this machine began before any hub described them, described.
async fn describe(
    node: &Arc<Node>,
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    network: &Network,
) -> Result<(), String> {
    let library = &assets.library;
    // Each account logged in here keeps its records' scope on this hub: the
    // hub writes its skills and settings there (design §28.17). Begun here
    // once, described as any scope begun here is.
    let logins: Vec<fastctx_types::AccountName> = link.view().me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
    for account in logins {
        // By when it was opened: one made again under the name has its own
        // (AR04, NN05).
        let Some(opened) = link.view().directory.account(&account).map(|found| found.opened) else { continue };
        let scope = wire::records_key(hub.as_str(), &account, opened.as_millisecond());
        let readers = Readers::Account { account };
        let classes = wire::classes_for(&readers, &scope);
        let proposal = Proposal { hub: hub.to_string(), readers, classes, born: None };
        let (network, proposal) = (network.clone(), serde_json::to_vec(&proposal).map_err(|error| error.to_string())?);
        blocking(library, move |library| {
            if library.holding(&scope)?.is_none() {
                library.propose_on(&network, &scope, proposal)?;
            }
            Ok::<_, fastctx_content::Error>(())
        })
        .await?;
    }
    // A project's ledger is not proposed: its entries name its machines and
    // their places, which a reader who cannot use a machine does not see;
    // held whole by every reader it would show them (M09). It waits for the
    // ledger split by project and machine, with the project's local reads.
    let projects: Vec<fastctx_types::access::TownId> = Vec::new();
    for town in projects {
        let readers = Readers::Project { town };
        let scope = format!("{}ledger", readers.key_prefix(hub.as_str()));
        let classes = wire::classes_for(&readers, &scope);
        let proposal = Proposal { hub: hub.to_string(), readers, classes, born: None };
        let (network, proposal) = (network.clone(), serde_json::to_vec(&proposal).map_err(|error| error.to_string())?);
        blocking(library, move |library| {
            if library.holding(&scope)?.is_none() {
                library.propose_on(&network, &scope, proposal)?;
            }
            Ok::<_, fastctx_content::Error>(())
        })
        .await?;
    }
    // Each is described apart: one the hub will not describe holds up none
    // after it, and one it said it will never describe — begun for an
    // account gone from it since, or for an earlier account of a name — is
    // kept as such and asked no more but as `undescribed` says. The pass
    // reports the first that failed otherwise.
    let dir = node.world.assets();
    let never = super::undescribed::read(&dir);
    // A hub that judges which account of a name a conversation was begun for
    // (`BORN`) is told it, and describes it for that one alone.
    let judged = link.asset_version() >= wire::BORN;
    let mut failed: Option<String> = None;
    let provisionals = blocking(library, |library| library.provisionals()).await?;
    // Kept as never to be described, but proposed no more — let go, or
    // described after all: not kept so any longer.
    let proposed: std::collections::BTreeSet<&str> = provisionals.iter().map(|(scope, _)| scope.as_str()).collect();
    let stale: Vec<String> = never.keys().filter(|scope| !proposed.contains(scope.as_str())).cloned().collect();
    for scope in stale {
        let dir = dir.clone();
        if let Err(error) = fastctx_platform::task::blocking(move || super::undescribed::forget(&dir, &scope)).await {
            node.log.line(format_args!("forgetting a scope no longer proposed: {error}"));
        }
    }
    for (scope, proposal) in provisionals {
        let Some(proposal) = proposal else { continue };
        // A conversation not on record here — deleted, or being — is never
        // described: what of it is here goes with it, not to its hub. One
        // deleted here is let go now; one merely not found — its place not
        // to be seen just now — is left as it is.
        if wire::of_conversation(&scope)
            && let Some(id) = scope.rsplit('/').next().and_then(|id| fastctx_types::ResidentId::try_from(id).ok())
        {
            let deleting = node.residents.is_deleting(&id);
            if !deleting && node.residents.about(&id).is_none() {
                if node.residents.deleted(&id).is_some() {
                    let key = scope.clone();
                    if blocking(library, move |library| library.revoke_proposed(&key)).await? && never.contains_key(&scope) {
                        let (dir, key) = (dir.clone(), scope.clone());
                        if let Err(error) = fastctx_platform::task::blocking(move || super::undescribed::forget(&dir, &key)).await {
                            node.log.line(format_args!("forgetting {scope}, let go with its conversation: {error}"));
                        }
                    }
                }
                continue;
            }
            if deleting {
                continue;
            }
        }
        let done: Result<(), String> = async {
            let Proposal { hub: meant, readers, classes, born } =
                serde_json::from_slice(&proposal).map_err(|error| format!("reading what was proposed: {error}"))?;
            if meant != hub.as_str() {
                return Ok(());
            }
            // Begun for an account of its name opened before the one there
            // now: never described, so never the later one's — the hub told
            // which one it was begun for, where it judges that (`BORN`), and
            // judged here again as the hub described it, below.
            let openers: Vec<fastctx_types::AccountName> = match &readers {
                Readers::Conversation { openers, .. } => openers.clone(),
                Readers::Body { opener, .. } => vec![opener.clone()],
                _ => Vec::new(),
            };
            let borns: Vec<(fastctx_types::AccountName, Option<i64>)> =
                openers.into_iter().map(|opener| { let born = born.or_else(|| born_under(node, &scope, hub, &opener)); (opener, born) }).collect();
            // When it was begun: an opener who it was begun for is not known
            // of is judged by it (`BEGUN_CLOCKS`) — the earlier of when its
            // conversation was born and when its scope was first held here.
            let begun_of = scope.clone();
            let begun = if wire::of_conversation(&scope) {
                let held = blocking(library, move |library| library.first_held(&begun_of)).await?;
                let born = scope
                    .rsplit('/')
                    .next()
                    .and_then(|id| fastctx_types::ResidentId::try_from(id).ok())
                    .and_then(|id| node.residents.about(&id))
                    .map(|(about, _)| about.born_at.as_millisecond());
                match (held, born) {
                    (Some(held), Some(born)) => Some(held.min(born)),
                    (held, born) => held.or(born),
                }
            } else {
                None
            };
            // Kept as never to be described: asked again once a day only
            // where the hub judges which account it was begun for, each
            // known or when it was begun — a later account of a name is
            // refused then, never given it.
            if let Some(kept) = never.get(&scope) {
                let judgeable =
                    judged && !borns.is_empty() && (begun.is_some() || borns.iter().all(|(_, born)| born.is_some()));
                if !judgeable || !super::undescribed::due(kept) {
                    return Ok(());
                }
            }
            // A hub that does not judge it is not asked while this machine
            // sees another account of an opener's name; nor is it kept as
            // never described for a directory perhaps not caught up yet.
            if !judged {
                for (opener, born) in &borns {
                    let now = link.view().directory.account(opener).map(|found| found.opened.as_millisecond());
                    if let (Some(born), Some(now)) = (born, now)
                        && *born != now
                    {
                        return Ok(());
                    }
                }
            }
            let said: std::collections::BTreeMap<String, String> = if judged {
                borns
                    .iter()
                    .filter_map(|(opener, born)| Some((format!("account:{opener}"), born.as_ref()?.to_string())))
                    .collect()
            } else {
                Default::default()
            };
            let conversation = scope.starts_with("c/");
            // The hub's own words for an opener it has not, or not as it was.
            let never_to_be = |why: &str| why.contains("there is no account:") || why.contains(wire::BEGUN_FOR_ANOTHER);
            let asked =
                Request::Describe { scope: scope.clone(), readers, classes, born: said, begun: if judged { begun } else { None } };
            let described = match ask(assets, link, asked).await {
                Ok(described) => described,
                // Revoked: its holding here ended (`heard`).
                Err(why) if why.starts_with(REVOKED) => return Ok(()),
                // The hub has no account it was begun for, or another of its
                // name: no hub will ever describe it for that one.
                Err(why) if wire::of_conversation(&scope) && never_to_be(&why) => return Err(format!("{GONE}{why}")),
                // A scope the hub does not describe for this machine that is
                // no conversation's — a project's, an account's, a log's: no
                // account here reads it any more — holds up no conversation's.
                Err(_) if !conversation => return Ok(()),
                Err(why) => return Err(why),
            };
            let Reply::Described { descriptor } = described else {
                return Err("the hub did not describe the scope".into());
            };
            if !assets.heard(hub, key, &descriptor.fact.0, &descriptor.word.0) {
                return Err(format!("the descriptor of {scope} is not the hub's"));
            }
            let read: wire::Descriptor = serde_json::from_slice(&descriptor.fact.0).map_err(|error| error.to_string())?;
            if read.scope != scope || read.hub != hub.as_str() {
                return Err(format!("the hub described another scope than {scope}"));
            }
            // Described for a later account of an opener's name than the one
            // it was begun for: not installed, so never that one's.
            for (opener, born) in &borns {
                let described = read.born.get(&format!("account:{opener}")).and_then(|opened| opened.parse::<i64>().ok());
                let later = match (born, described) {
                    (Some(born), Some(described)) => *born != described,
                    // Who it was begun for not known: an account opened after
                    // it was begun here did not begin it.
                    (None, Some(described)) => begun.is_some_and(|begun| described > begun.saturating_add(wire::BEGUN_CLOCKS)),
                    _ => false,
                };
                if later {
                    return Err(format!("{GONE}the hub described it for a later account named {opener}"));
                }
            }
            let (network, key) = (network.clone(), scope.clone());
            let installed =
                blocking(library, move |library| library.install_proposed(&network, &key, descriptor.fact.0, read.seq as i64))
                    .await?;
            // Let go meanwhile, its conversation deleted: not held anew.
            if installed.is_some() && never.contains_key(&scope) {
                let (dir, key) = (dir.clone(), scope.clone());
                fastctx_platform::task::blocking(move || super::undescribed::forget(&dir, &key))
                    .await
                    .map_err(|error| format!("keeping that {scope} was described: {error}"))?;
            }
            Ok(())
        }
        .await;
        let Err(why) = done else { continue };
        match why.strip_prefix(GONE) {
            Some(why) => {
                let (dir, kept, said) = (node.world.assets(), scope.clone(), why.to_string());
                match fastctx_platform::task::blocking(move || super::undescribed::keep(&dir, &kept, &said)).await {
                    Ok(()) => node.log.line(format_args!("{scope} will never be described by {hub}: {why}; it is asked no more")),
                    Err(error) => {
                        failed.get_or_insert(format!("keeping that {scope} will never be described: {error}"));
                    }
                }
            }
            None => {
                failed.get_or_insert(format!("describing {scope}: {why}"));
            }
        }
    }
    failed.map_or(Ok(()), Err)
}

/// When the account `opener` the conversation whose scope is `scope` was
/// begun for on `hub` was opened, in milliseconds, as the conversation
/// keeps it; none where it does not.
fn born_under(node: &Node, scope: &str, hub: &HubId, opener: &fastctx_types::AccountName) -> Option<i64> {
    let id = fastctx_types::ResidentId::try_from(scope.rsplit('/').next()?).ok()?;
    let (about, _) = node.residents.about(&id)?;
    about.born_as?.into_iter().find(|born| born.hub == *hub && born.account == *opener).map(|born| born.generation.as_millisecond())
}

/// How often the scopes a hub lists for this machine are listed whole: a
/// change of the directory may let it hold one described long before.
const RELIST: Duration = Duration::from_secs(300);

/// Installs each scope the hub says this machine may hold that it does not
/// hold yet, or holds under another descriptor (design §6.6), the
/// descriptor the hub's word: those described since the last listing, or
/// all of them when the driver begins, every `RELIST` and as what decides
/// holding changes. Each installed is pulled whole before the change
/// numbers carry it (§28.1). All it listed, when it listed all under one
/// basis.
async fn discover(
    node: &Arc<Node>,
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    network: &Network,
) -> Result<Option<std::collections::BTreeSet<String>>, String> {
    let mut listed_all = std::collections::BTreeSet::new();
    let began = assets.forgotten.load(std::sync::atomic::Ordering::SeqCst);
    let known = assets.listed.lock().expect("listed").get(hub).cloned();
    // No basis kept — the last listing unsettled, or told none — lists all.
    let (mut after, mut whole, heard) = match known {
        Some(listed) if listed.whole.elapsed() < RELIST && !listed.basis.is_empty() => {
            (listed.through, None, Some(listed.basis))
        }
        _ => (None, Some(std::time::Instant::now()), None),
    };
    // What the listing went by. Changed before it began, all is listed
    // from the first. Changed while it goes on, the listing goes on to its
    // end — it never starts over, however often what decides holding
    // changes (AE07) — and the next pass lists all again, a scope passed by
    // being perhaps this machine's now (T10, BB12).
    let mut basis: Option<String> = None;
    let mut unsettled = false;
    loop {
        let Reply::Holdings { scopes, next, through, basis: said } =
            ask(assets, link, Request::Holdings { after: after.clone() }).await?
        else {
            return Err("the hub did not list what this machine may hold".into());
        };
        // A page told no basis was judged by a directory other than the one
        // it was made by (AF05).
        if said.is_empty() {
            unsettled = true;
        }
        match &basis {
            Some(basis) if *basis != said => unsettled = true,
            Some(_) => {}
            None if whole.is_none() && heard.as_deref() != Some(said.as_str()) => {
                whole = Some(std::time::Instant::now());
                after = None;
                basis = Some(said);
                continue;
            }
            None => {}
        }
        if basis.is_none() {
            basis = Some(said);
        }
        for held in scopes {
            if !assets.heard(hub, key, &held.descriptor.fact.0, &held.descriptor.word.0) {
                return Err(format!("the descriptor of {} is not the hub's", held.scope));
            }
            let read: wire::Descriptor =
                serde_json::from_slice(&held.descriptor.fact.0).map_err(|error| error.to_string())?;
            if read.scope != held.scope || read.hub != hub.as_str() {
                return Err(format!("the hub listed another scope than {}", held.scope));
            }
            listed_all.insert(held.scope.clone());
            // Installed as listed, and what it may hold of it and who logged
            // in here reads it kept with it (HH04, AV01) — but where its copy
            // was let go here at its owner's word and every reader of it here
            // stays paused (`copies::clear`), this machine began it and
            // describes it itself, or its conversation is deleted here.
            let holds = held.holds.join("\n");
            let said = serde_json::json!({ "of": held.readers, "frozen": held.frozen }).to_string();
            let (asker, scope, readers, hub_text) = (node.clone(), held.scope.clone(), held.readers.clone(), hub.to_string());
            let (network, fact, revision) = (network.clone(), held.descriptor.fact.0, read.seq as i64);
            fastctx_platform::task::blocking(move || {
                super::copies::hold_listed(&asker, &hub_text, &scope, &readers, (&network, fact, revision), (&holds, &said))
            })
            .await?;
        }
        // Where the next listing goes on from: the last scope looked at,
        // given or not (AD10).
        if through.is_some() {
            after = through;
        }
        match next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    let mut listed = assets.listed.lock().expect("listed");
    // Forgotten while it went on — access revoked, the session ended — it
    // is not kept: the next pass lists all (CC07).
    if assets.forgotten.load(std::sync::atomic::Ordering::SeqCst) != began {
        return Ok(None);
    }
    let at = whole.or_else(|| listed.get(hub).map(|listed| listed.whole)).unwrap_or_else(std::time::Instant::now);
    // Unsettled, no basis is kept: the next pass lists all.
    let basis = if unsettled { String::new() } else { basis.unwrap_or_default() };
    listed.insert(hub.clone(), crate::assets::Listed { through: after, whole: at, basis });
    Ok((whole.is_some() && !unsettled).then_some(listed_all))
}

/// The frames of this machine's own streams of `scope` past what the hub
/// has.
async fn upload(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    session: u64,
    scope: &str,
) -> Result<(), String> {
    let library = &assets.library;
    // What a refusal took out goes again first, the hub asked for a new
    // epoch where none was opened yet (a pass cut short after the refusal).
    let scope_text = scope.to_string();
    let mut waiting_failed: Option<String> = None;
    for (stream, cut, renewed) in blocking(library, move |library| library.waiting(&scope_text)).await? {
        let done = async {
            // A stream this machine keeps (`direct`) takes its new epoch as
            // any does: until it has, nothing of it from where it was cut is
            // given to a machine that holds a copy (`served`), so what is
            // cut away here was with no other machine.
            if !renewed {
                renew(assets, hub, link, key, scope, &stream, cut).await?;
            }
            // Only into an epoch this machine orders: one another took over
            // takes none of this one's writes (AG08).
            let (scope_text, stream_text, device) = (scope.to_string(), stream.clone(), assets.device());
            if blocking(library, move |library| library.orderer(&scope_text, &stream_text))
                .await?
                .is_some_and(|orderer| orderer != device)
            {
                return Err("another device orders the stream now; what waits is not committed again".to_string());
            }
            resubmit(assets, scope, &stream).await
        }
        .await;
        if let Err(why) = done {
            if why.starts_with(REVOKED) {
                return Err(why);
            }
            waiting_failed.get_or_insert(format!("{stream}: {why}"));
        }
    }
    let device = assets.device();
    let key_text = scope.to_string();
    // With a hub that keeps asset lines, frames go up on one, each whole and
    // many to an exchange (`lines`).
    let lines = link.keeps_asset_lines();
    let (page, turn, run_frames, run_bytes) = if lines {
        (
            super::lines::FRAMES_UP_AT_ONCE,
            super::lines::FRAMES_UP_A_TURN,
            fastctx_types::asset_lines::FRAMES,
            fastctx_types::asset_lines::FRAME_BYTES,
        )
    } else {
        (FRAMES_AT_ONCE, FRAMES_UP_A_TURN, wire::FRAMES_A_TAKE, wire::MANY_BYTES)
    };
    // A stream that fails holds up none after it; the scope reports the first.
    let mut failed: Option<String> = waiting_failed;
    for (stream, from) in blocking(library, move |library| library.uploads(&key_text, &device)).await? {
        // What this machine orders of a conversation stays here (`direct`):
        // the hub is told only where it stands, below.
        if super::direct::stays(link, scope, &stream) {
            continue;
        }
        let _step = phase(hub, &format!("going up {stream} of {scope} from {from}"));
        let done: Result<(), String> = async {
            let mut from = from;
            let mut gone = 0usize;
            loop {
                let (scope_text, stream_text) = (scope.to_string(), stream.clone());
                // No more of them than a couple of runs' worth is read at once.
                let frames = blocking(library, move |library| {
                    library.export_within(&scope_text, &stream_text, from, page, run_bytes * 2)
                })
                .await?;
                if frames.is_empty() {
                    break;
                }
                // Frames that begin past where the stream goes up from — what
                // came before went with a holding of it that ended while the
                // stream stayed on this machine (`direct`) — have nothing the
                // hub can take them after: said, and none of them sent, lest
                // the same frames go up again each time the hub answers with
                // what it still lacks before them.
                if let Some(first) = frames.first().and_then(|frame| Header::decode(&frame.header).ok()).map(|header| header.first)
                    && first > from
                {
                    return Err(format!(
                        "{stream} of {scope}: its frames here begin at {first}, past {from} where it goes up from; what came before them is not on this machine"
                    ));
                }
                if gone >= turn {
                    assets.more.lock().expect("more").insert(hub.clone());
                    return Err(format!("{GOES_ON}{stream} of {scope} going up from {from}"));
                }
                gone += frames.len();
                // Frames the control connection carries go up several in one
                // take where the hub speaks it (E2): it writes them with one
                // disk synchronization. One that wants objects or proof goes
                // again alone, as one travelling apart always does.
                let many = link.asset_version() >= wire::MANY;
                let mut queue: std::collections::VecDeque<(Header, wire::Frame, bool)> = std::collections::VecDeque::new();
                // A frame whose header does not read ends the page there: those
                // before it go up first, as one at a time they did.
                let mut unreadable = None;
                for frame in frames {
                    match Header::decode(&frame.header) {
                        Ok(header) => queue.push_back((header, wire::Frame::whole(frame.header, frame.signature, frame.body), false)),
                        Err(error) => {
                            unreadable = Some(error.to_string());
                            break;
                        }
                    }
                }
                while let Some((header, frame, alone)) = queue.pop_front() {
                    if many && !alone && (lines || !frame.too_large()) {
                        let mut run = vec![(header, frame)];
                        let mut bytes = run[0].1.body.0.len();
                        while run.len() < run_frames
                            && let Some((_, next, false)) = queue.front()
                            && (lines || !next.too_large())
                            && bytes + next.body.0.len() <= run_bytes
                        {
                            bytes += next.body.0.len();
                            let (header, frame, _) = queue.pop_front().expect("looked at");
                            run.push((header, frame));
                        }
                        if run.len() > 1 {
                            let going: Vec<wire::Frame> = run.iter().map(|(_, frame)| frame.clone()).collect();
                            let taken = if lines {
                                super::lines::take(assets, link, scope, &going).await.map(|received| Reply::ReceivedMany { received })
                            } else {
                                ask(assets, link, Request::TakeMany { scope: scope.to_string(), frames: going }).await
                            };
                            let received = match taken {
                                Ok(Reply::ReceivedMany { received }) => received,
                                Ok(_) => return Err("the hub did not take the frames".into()),
                                // Refused whole — its first frame did not pass:
                                // that one goes alone, and hears why as ever.
                                Err(why) if why.starts_with(REVOKED) => return Err(why),
                                Err(_) => {
                                    let mut run = run.into_iter();
                                    let (header, frame) = run.next().expect("a run of frames");
                                    for (header, frame) in run.rev() {
                                        queue.push_front((header, frame, false));
                                    }
                                    queue.push_front((header, frame, true));
                                    continue;
                                }
                            };
                            if received.is_empty() || received.len() > run.len() {
                                return Err("the hub's word on the frames is not one for each".into());
                            }
                            let mut rest = run.into_iter();
                            let mut back = Vec::new();
                            for received in received {
                                let (header, frame) = rest.next().expect("one word for each");
                                match went(assets, hub, key, session, scope, &stream, &header, received).await? {
                                    Went::On => from = header.first + u64::from(header.count),
                                    Went::Refused(cut) => {
                                        refused(assets, hub, link, key, scope, &stream, cut).await?;
                                        return Ok(());
                                    }
                                    Went::Gap(wanted) => {
                                        from = wanted;
                                        queue.clear();
                                        rest = Vec::new().into_iter();
                                        break;
                                    }
                                    Went::Wants => {
                                        back.push((header, frame, true));
                                        break;
                                    }
                                }
                            }
                            // What the hub did not take goes again, in order.
                            back.extend(rest.map(|(header, frame)| (header, frame, false)));
                            for again in back.into_iter().rev() {
                                queue.push_front(again);
                            }
                            continue;
                        }
                        let (single_header, single_frame) = run.pop().expect("one frame");
                        queue.push_front((single_header, single_frame, true));
                        continue;
                    }
                    // Objects the hub has not, or wants proved, go first; then
                    // the frame again, naming what this machine proved.
                    let mut proven: Vec<Bytes> = Vec::new();
                    let (mut rounds, mut staged) = (0, 0);
                    let received = loop {
                        let Reply::Received { received } =
                            super::apart::take(assets, link, scope, &frame, &proven, &mut staged).await?
                        else {
                            return Err("the hub did not take the frame".into());
                        };
                        let (objects, prove) = match &received {
                            wire::Received::Missing { objects } => (objects, false),
                            wire::Received::Prove { objects } => (objects, true),
                            _ => break received,
                        };
                        rounds += 1;
                        if rounds > OBJECT_ROUNDS {
                            return Err(format!("{stream} of {scope}: the hub still wants objects after they went"));
                        }
                        proven.extend(super::objects::put(library, link, scope, objects, prove).await?);
                    };
                    match went(assets, hub, key, session, scope, &stream, &header, received).await? {
                        Went::On => {}
                        Went::Refused(cut) => {
                            refused(assets, hub, link, key, scope, &stream, cut).await?;
                            return Ok(());
                        }
                        Went::Gap(wanted) => {
                            from = wanted;
                            continue;
                        }
                        Went::Wants => return Err(format!("{stream} of {scope}: the hub still wants objects after they went")),
                    }
                    from = header.first + u64::from(header.count);
                }
                if let Some(why) = unreadable {
                    return Err(format!("{stream} of {scope}: a frame's header does not read: {why}"));
                }
            }
            Ok(())
        }
        .await;
        if let Err(why) = done {
            if why.starts_with(REVOKED) {
                return Err(why);
            }
            failed.get_or_insert(format!("{stream}: {why}"));
        }
    }
    // The hub is told where the streams kept here stand (`direct`); not
    // told, the scope goes up again the next pass.
    if let Err(why) = super::direct::announce(assets, hub, link, key, scope).await {
        if why.starts_with(REVOKED) {
            return Err(why);
        }
        failed.get_or_insert(why);
    }
    failed.map_or(Ok(()), Err)
}

/// What became of a frame of a stream that went up, by the hub's word on it.
enum Went {
    /// Applied or parked, its receipt kept: on to the next.
    On,
    /// Refused, the hub's word kept: the stream goes back before `cut`.
    Refused(u64),
    /// The hub wants the stream from the frame at this sequence.
    Gap(u64),
    /// The hub wants objects first, or them proved.
    Wants,
}

/// Keeps the hub's word `received` on the frame `header` of `stream`, as a
/// receipt where it gives one, and says what became of the frame.
#[allow(clippy::too_many_arguments)]
async fn went(
    assets: &Assets,
    hub: &HubId,
    key: &PublicKey,
    session: u64,
    scope: &str,
    stream: &str,
    header: &Header,
    received: wire::Received,
) -> Result<Went, String> {
    let receipt = |said: Said, word: Bytes| Receipt {
        stream: stream.to_string(),
        epoch: header.epoch,
        first: header.first,
        digest: header.digest,
        said,
        signature: word.0,
    };
    match received {
        wire::Received::Applied { change, prefix, word, .. } => {
            fastctx_platform::lab::die_at("receipt", stream);
            let said = Said::Applied { change, prefix: array(&prefix, "a receipt's prefix")? };
            keep(assets, hub, key, session, scope, receipt(said, word)).await?;
            Ok(Went::On)
        }
        wire::Received::Parked { sequence, accumulated, word } => {
            let said = Said::Parked { sequence, accumulated: array(&accumulated, "a receipt's accumulator")? };
            keep(assets, hub, key, session, scope, receipt(said, word)).await?;
            Ok(Went::On)
        }
        wire::Received::Rejected { from: cut, word, .. } => {
            keep(assets, hub, key, session, scope, receipt(Said::Rejected { from: cut }, word)).await?;
            Ok(Went::Refused(cut))
        }
        wire::Received::Gap { from: wanted, .. } => Ok(Went::Gap(wanted)),
        wire::Received::Missing { .. } | wire::Received::Prove { .. } => Ok(Went::Wants),
    }
}

/// Keeps the hub's word on a frame, when it is the pinned key's.
async fn keep(
    assets: &Assets,
    hub: &HubId,
    key: &PublicKey,
    session: u64,
    scope: &str,
    receipt: Receipt,
) -> Result<(), String> {
    let frame = fastctx_content::v2::FrameKey {
        scope,
        stream: &receipt.stream,
        epoch: receipt.epoch,
        first: receipt.first,
        digest: &receipt.digest,
    };
    let statement = match &receipt.said {
        Said::Applied { change, prefix } => Some(fastctx_content::v2::applied_statement(&frame, *change, prefix)),
        Said::Parked { sequence, accumulated } => {
            Some(fastctx_content::v2::parked_statement(&frame, *sequence, accumulated))
        }
        Said::Rejected { .. } => None,
    };
    if let Some(statement) = statement
        && !assets.heard(hub, key, &statement, &receipt.signature)
    {
        return Err("the hub's receipt is not under its service key".into());
    }
    let scope = scope.to_string();
    blocking(&assets.library, move |library| library.receipt(&scope, receipt, session)).await
}

/// The hub refused the frame of `stream` at `cut`: the stream goes back
/// before it, the hub opens a new epoch for this machine there, and what
/// the refusal took out is committed again in it. A pass cut short
/// anywhere here takes it up again (`Library::waiting`).
async fn refused(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    scope: &str,
    stream: &str,
    cut: u64,
) -> Result<(), String> {
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    blocking(&assets.library, move |library| library.rejected(&scope_text, &stream_text, cut)).await?;
    renew(assets, hub, link, key, scope, stream, cut).await?;
    resubmit(assets, scope, stream).await
}

/// The hub opens a new epoch of `stream` for this machine at `cut`, where
/// the stream was cut back to: what was committed into it since goes with
/// what the refusal took out.
async fn renew(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    scope: &str,
    stream: &str,
    cut: u64,
) -> Result<(), String> {
    let library = &assets.library;
    let scope_text = scope.to_string();
    let next = blocking(library, move |library| library.streams(&scope_text))
        .await?
        .into_iter()
        .find(|(name, _)| name == stream)
        .map_or(0, |(_, next)| next);
    if next > cut {
        let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
        blocking(library, move |library| library.rejected(&scope_text, &stream_text, cut)).await?;
    }
    // The chain value the new epoch follows: where the stream stands now.
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    let standing = blocking(library, move |library| library.standing(&scope_text)).await?;
    let head = standing
        .into_iter()
        .find(|(name, sequence, _)| name == &stream_text && sequence + 1 == cut)
        .map_or([0u8; 32], |(_, _, head)| head);
    let request = Request::Epoch { scope: scope.to_string(), stream: stream.to_string(), next: cut, head: head.into() };
    let Reply::Epoch { descriptor } = ask(assets, link, request).await? else {
        return Err("the hub did not open a new epoch".into());
    };
    if !assets.heard(hub, key, &descriptor.fact.0, &descriptor.word.0) {
        return Err("the new epoch's descriptor is not the hub's".into());
    }
    let fact = EpochFact::decode(&descriptor.fact.0)?;
    if fact.scope != scope || fact.stream != stream || fact.start != cut {
        return Err("the hub opened another epoch than the one asked for".into());
    }
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    // What was committed from the cut on while the hub was asked is cut
    // away with the epoch's opening, so the epoch begins where it was signed
    // — on the chain value it was signed to follow, checked in the same
    // transaction.
    let follows = fact.follows;
    blocking(library, move |library| {
        library.new_epoch_following(&scope_text, &stream_text, fact.epoch, descriptor.fact.0, cut, Some(follows))
    })
    .await
}

/// Commits what waits of `stream` again, in its current epoch.
async fn resubmit(assets: &Assets, scope: &str, stream: &str) -> Result<(), String> {
    for _ in 0..RESUBMIT_ROUNDS {
        let (scope_text, stream_text, signer) = (scope.to_string(), stream.to_string(), assets.signer());
        let committed =
            blocking(&assets.library, move |library| library.resubmit(&scope_text, &stream_text, 64, signer)).await?;
        if committed == 0 {
            return Ok(());
        }
    }
    // A stream written to as fast as what waits is looked at goes on the
    // next pass, not this one for good: failed, the scope stays among those
    // to go up (`turn`).
    Err(format!("more of {stream} waits to be committed again than one pass commits; the next pass goes on"))
}

/// The most rounds of committing again one stream's waiting records a pass
/// makes.
const RESUBMIT_ROUNDS: usize = 256;

/// The frames of the streams of `scope` this machine holds, past what it
/// has, under the hub's signed epochs and gap facts.
/// Takes the frames of the streams of `scope` this machine holds past what
/// it has: the streams the hub lags on, and the first stream's failure, if
/// any (one fails holding up no other).
#[allow(clippy::too_many_arguments)]
async fn pull(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    session: u64,
    scope: &str,
    holding: i64,
    isolated: bool,
) -> Result<(std::collections::BTreeSet<String>, Option<String>), String> {
    let library = &assets.library;
    let Reply::Streams { streams, withheld } = ask(assets, link, Request::Streams { scope: scope.to_string() }).await?
    else {
        return Err("the hub did not list the scope's streams".into());
    };
    let scope_text = scope.to_string();
    let local: std::collections::BTreeMap<String, u64> =
        blocking(library, move |library| library.streams(&scope_text)).await?.into_iter().collect();
    // A stream this machine may hold no more goes from here (§2.3); of its
    // own, what the hub has not taken is kept first, to commit again, never
    // dropped with the rest. One not begun here, or empty, is withheld all
    // the same (U05): a first write on its way finds it so.
    let scope_text = scope.to_string();
    let already = blocking(library, move |library| library.withheld_streams(&scope_text)).await?;
    let (scope_text, device) = (scope.to_string(), assets.device());
    let mine: std::collections::BTreeSet<String> =
        blocking(library, move |library| library.own_streams(&scope_text, &device)).await?.into_iter().collect();
    for stream in
        withheld.into_iter().filter(|stream| local.get(stream).is_some_and(|at| *at > 0) || !already.contains(stream))
    {
        // What this machine orders and keeps of a conversation (`direct`)
        // is with no other machine to come back from, and what waited of
        // it could only be committed again past what the copies hold: it
        // is not let go for a hub that holds none of it.
        if super::direct::stays(link, scope, &stream) && mine.contains(&stream) {
            continue;
        }
        let (scope_text, device) = (scope.to_string(), assets.device());
        blocking(library, move |library| library.withhold(&scope_text, &stream, &device)).await?;
    }
    // The streams the hub lags on: given back here, not checked against it.
    let mut ahead = std::collections::BTreeSet::new();
    let mut gave_back: Option<String> = None;
    // What the hub lacks that this machine has — the hub recovered to
    // before it (design §17) — goes back: its own streams' uploading goes on
    // from where the hub is, a replica's frames are given again.
    let theirs: std::collections::BTreeMap<String, u64> =
        streams.iter().map(|at| (at.stream.clone(), at.next)).collect();
    // What this machine wrote of a stream before the hub described the
    // scope, the hub never having taken any of it, while the hub had the
    // stream from another machine all along — a conversation come from it,
    // begun here in vain — goes: it is made again once the stream is handed
    // over here, and followed as a replica's till then (AH01, LL11).
    let mut local: std::collections::BTreeMap<String, u64> = if discard_unaccepted(assets, hub, link, key, scope, &theirs).await? {
        let scope_text = scope.to_string();
        blocking(library, move |library| library.streams(&scope_text)).await?.into_iter().collect()
    } else {
        local
    };
    let (scope_text, device) = (scope.to_string(), assets.device());
    let own: std::collections::BTreeSet<String> =
        blocking(library, move |library| library.own_streams(&scope_text, &device)).await?.into_iter().collect();
    let mut cut_back = Vec::new();
    for (stream, at) in &local {
        let hub_at = theirs.get(stream).copied();
        // A stream kept by the machine that orders it (`direct`): the hub
        // holding less of it is as it is meant to be. Nothing of it goes
        // back to the hub or is cut to where the hub stands, and where it
        // stands is not checked against the hub (`cursors`).
        if super::direct::stays(link, scope, stream) {
            if !own.contains(stream) {
                ahead.insert(stream.clone());
            }
            continue;
        }
        if own.contains(stream) {
            let hub_at = hub_at.unwrap_or(0);
            if *at > hub_at {
                let (scope_text, stream_text) = (scope.to_string(), stream.clone());
                blocking(library, move |library| library.rewind_uploads(&scope_text, &stream_text, hub_at)).await?;
                // Gone up again from where the hub is, this very pass
                // (`pass`), and until it has.
                assets.uploads.lock().expect("uploads").insert(scope.to_string());
            }
        } else if *at > hub_at.unwrap_or(0) && fastctx_content::v2::hub_orders(stream) {
            // A stream the hub orders, longer here than the hub has it after
            // a recovery: the hub writes it again from its records, and this
            // replica follows it (AS02, AS07). What the hub no longer has of
            // it is kept aside here whole, never lost: its records may have
            // gone from the hub's sources since (AT02); it is cut back at
            // one of its own frames' ends (AT03).
            let (scope_text, stream_text, from) = (scope.to_string(), stream.clone(), hub_at.unwrap_or(0));
            let next = blocking(library, move |library| {
                library.set_aside(&scope_text, &stream_text, from, "the hub, put back, holds the stream only before this")
            })
            .await?;
            // Pulled on from where it was cut back to, this very pass (QQ04).
            cut_back.push((stream.clone(), next));
        } else if *at > hub_at.unwrap_or(0) {
            // A replica's frames the hub lost, all of the stream or its
            // tail, given again; a stream that fails holds up no other.
            ahead.insert(stream.clone());
            if let Err(why) = resupply(assets, hub, link, key, session, scope, stream, hub_at.unwrap_or(0)).await {
                if why.starts_with(REVOKED) {
                    return Err(why);
                }
                // Given back again later: a pass that failed it looks at
                // the scope again (HH06).
                gave_back.get_or_insert(format!("giving {stream} back to the hub: {why}"));
            }
        }
    }
    local.extend(cut_back);
    let mut failed: Option<String> = gave_back;
    // A stream handed on since — this machine's own among them — though no
    // frame of it moved: its descriptors are taken, as the change numbers
    // would have (I09); a look whole begins where they leave off (LL05).
    for at in &streams {
        let (scope_text, stream_text) = (scope.to_string(), at.stream.clone());
        let known = blocking(library, move |library| library.known_last_epoch(&scope_text, &stream_text)).await?;
        if at.epoch > known.unwrap_or(0)
            && let Err(why) = succession(assets, hub, link, key, scope, &at.stream).await
        {
            failed.get_or_insert(format!("{}: {why}", at.stream));
        }
    }
    // Isolated, the hub's frames do not come down to this disk.
    let streams = if isolated { Vec::new() } else { streams };
    // Where each machine that orders streams of the scope says they stand
    // (`direct`), asked once a pull; none for one that did not answer.
    let mut kept: Kept = Kept::new();
    for wire::StreamAt { stream, next, .. } in streams {
        let at = local.get(&stream).copied().unwrap_or(0);
        let pulled = if super::direct::stays(link, scope, &stream) {
            // Its own, a stream kept here comes from nowhere.
            if own.contains(&stream) {
                continue;
            }
            match pull_kept(assets, hub, link, key, scope, &stream, holding, at, next, &mut kept).await {
                Ok(true) => Ok(()),
                Ok(false) => Err(format!("{UNREACHED_NOW}{stream}")),
                Err(why) => Err(why),
            }
        } else {
            pull_stream(assets, hub, link, key, scope, &stream, holding, at, next, None, None).await
        };
        if let Err(why) = pulled {
            if why.starts_with(REVOKED) {
                return Err(why);
            }
            // Paused part way, or its machine away just now: said as it is,
            // so it is not taken whole.
            if why.starts_with(PAUSED) || why.starts_with(UNREACHED_NOW) {
                failed.get_or_insert(why);
                continue;
            }
            failed.get_or_insert(format!("{stream}: {why}"));
        }
    }
    // A stream the hub gives this machine to hold again, withheld before, is
    // committed to again once it holds again what the hub has of it: a
    // commit before would take a place the hub's own frame holds (BB01).
    let scope_text = scope.to_string();
    let now: std::collections::BTreeMap<String, u64> =
        blocking(library, move |library| library.streams(&scope_text)).await?.into_iter().collect();
    let caught_up: Vec<String> = theirs
        .iter()
        .filter(|(stream, next)| now.get(*stream).copied().unwrap_or(0) >= **next)
        .map(|(stream, _)| stream.clone())
        .collect();
    let scope_text = scope.to_string();
    blocking(library, move |library| library.hold_again(&scope_text, holding, caught_up)).await?;
    Ok((ahead, failed))
}

/// Where the machines that order streams of a scope say they stand, by
/// machine: asked once while the scope is pulled; none for a machine that
/// did not answer.
type Kept = std::collections::HashMap<fastctx_types::MachineName, Option<std::collections::HashMap<String, u64>>>;

/// Takes the replica `stream` of `scope`, one the machine that orders it
/// keeps (`direct`): from that machine, as far as it says the stream
/// stands; and where that machine gives nothing now, from the hub as far
/// as the hub still holds it. A machine that does not answer fails nothing:
/// the stream is taken when it does, told by the hub or found at a look.
#[allow(clippy::too_many_arguments)]
async fn pull_kept(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    scope: &str,
    stream: &str,
    holding: i64,
    at: u64,
    hub_next: u64,
    kept: &mut Kept,
) -> Result<bool, String> {
    let library = &assets.library;
    // Whether its machine was heard as far as it stands: away, what the hub
    // holds is taken, and the stream is not taken for whole.
    let mut reached = true;
    // Who orders it is known by its epochs, which the hub signs.
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    if blocking(library, move |library| library.known_last_epoch(&scope_text, &stream_text)).await?.is_none() {
        succession(assets, hub, link, key, scope, stream).await?;
    }
    if let Some(machine) = super::direct::source(assets, hub, link, scope, stream).await {
        if !kept.contains_key(&machine) {
            let heads = super::direct::heads(assets, hub, link, &machine, vec![scope.to_string()]).await;
            // Of this scope alone: a machine's word on another is not heard.
            kept.insert(
                machine.clone(),
                heads.ok().map(|heads| heads.into_iter().filter(|head| head.scope == scope).map(|head| (head.stream, head.next)).collect()),
            );
        }
        if matches!(kept.get(&machine), Some(None)) {
            reached = false;
        }
        // Standing no further there than here, what the hub still holds
        // past here is taken below all the same.
        if let Some(Some(theirs)) = kept.get(&machine)
            && let Some(next) = theirs.get(stream).copied()
            && next > at
        {
            assets.direct.heard(scope, stream, next);
            match pull_stream(assets, hub, link, key, scope, stream, holding, at, next, None, Some(&machine)).await {
                // What the hub holds past where its machine stands — a
                // machine set back, a stream handed on — is taken below.
                Ok(()) => {}
                Err(why) if why.starts_with(REVOKED) || why.starts_with(PAUSED) || why.starts_with(GOES_ON) => return Err(why),
                // Its machine gave nothing now: taken from the hub below as
                // far as the hub holds it; the rest waits for its machine.
                Err(why) if why.starts_with(super::direct::UNREACHED) => reached = false,
                // What its machine gave could not be kept: said, as what the
                // hub gives that cannot be kept is.
                Err(why) => return Err(why),
            }
        }
    }
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    let at = blocking(library, move |library| library.streams(&scope_text))
        .await?
        .into_iter()
        .find(|(name, _)| *name == stream_text)
        .map_or(0, |(_, next)| next);
    if at >= hub_next {
        return Ok(reached);
    }
    pull_stream(assets, hub, link, key, scope, stream, holding, at, hub_next, None, None).await.map(|()| reached)
}

/// Takes the hub's signed epochs and gap facts of `stream` of `scope`.
async fn succession(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, stream: &str) -> Result<(), String> {
    let library = &assets.library;
    let Reply::Succession { epochs, gaps } =
        ask(assets, link, Request::Succession { scope: scope.to_string(), stream: stream.to_string() }).await?
    else {
        return Err("the hub did not give the stream's succession".into());
    };
    let (device, last) = (assets.device(), epochs.len().saturating_sub(1));
    for (index, fact) in epochs.into_iter().enumerate() {
        let signed = fastctx_content::v2::Signed { fact: fact.fact.0, signature: fact.word.0 };
        let scope_text = scope.to_string();
        let verifier = Pinning { assets, hub, key: *key };
        // The verifier borrows this machine's state: checked here, kept on the library's thread.
        if !fastctx_content::v2::HubVerifier::verify(&verifier, &signed.fact, &signed.signature) {
            return Err("an epoch's descriptor is not the hub's".into());
        }
        // The last epoch, this machine's own — handed over to it, the answer
        // lost — is opened as a takeover opens it, once the stream here
        // reaches where it begins; until then nothing of it is known here,
        // and the stream is followed as a replica's (AP03).
        if index == last && EpochFact::decode(&signed.fact).is_ok_and(|fact| fact.device == device) {
            if blocking(library, move |library| library.take_over(&scope_text, signed, &Trusted)).await?
                == fastctx_content::v2::Taken::Opened
            {
                assets.taken_over.lock().expect("taken over").push(scope.to_string());
            }
            continue;
        }
        blocking(library, move |library| library.accept_epoch(&scope_text, signed, &Trusted)).await?;
    }
    for fact in gaps {
        let signed = fastctx_content::v2::Signed { fact: fact.fact.0, signature: fact.word.0 };
        let verifier = Pinning { assets, hub, key: *key };
        if !fastctx_content::v2::HubVerifier::verify(&verifier, &signed.fact, &signed.signature) {
            return Err("a gap fact is not the hub's".into());
        }
        let scope_text = scope.to_string();
        blocking(library, move |library| library.accept_gap(&scope_text, signed, &Trusted)).await?;
    }
    Ok(())
}

/// Lets go what this machine wrote of the streams of `scope` the hub never
/// took anything of, where the hub's first epoch of the stream is another
/// device's (AH01): whether any went.
async fn discard_unaccepted(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    scope: &str,
    theirs: &std::collections::BTreeMap<String, u64>,
) -> Result<bool, String> {
    let library = &assets.library;
    let (scope_text, device) = (scope.to_string(), assets.device());
    let unaccepted = blocking(library, move |library| library.unaccepted(&scope_text, &device)).await?;
    let mut discarded = false;
    for (stream, _) in unaccepted {
        // The hub knows nothing of it: this machine's to send up. A stream
        // it lists, though it holds no frame of it — opened by its source,
        // which keeps its frames (`assets::DIRECT`) — is the device's its
        // first epoch names, asked below.
        if !theirs.contains_key(&stream) {
            continue;
        }
        let Reply::Succession { epochs, .. } =
            ask(assets, link, Request::Succession { scope: scope.to_string(), stream: stream.clone() }).await?
        else {
            return Err("the hub did not give the stream's succession".into());
        };
        let Some(first) = epochs.first() else { continue };
        let verifier = Pinning { assets, hub, key: *key };
        if !fastctx_content::v2::HubVerifier::verify(&verifier, &first.fact.0, &first.word.0) {
            return Err("an epoch's descriptor is not the hub's".into());
        }
        if EpochFact::decode(&first.fact.0)?.device == device {
            continue;
        }
        let checkpoint = crate::timeline::checkpoint_of(scope.rsplit('/').next().unwrap_or(scope));
        let (scope_text, stream_text) = (scope.to_string(), stream.clone());
        let signed = fastctx_content::v2::Signed { fact: first.fact.0.clone(), signature: first.word.0.clone() };
        discarded |= blocking(library, move |library| {
            library.discard_unaccepted(&scope_text, &stream_text, &device, Some(&checkpoint), Some(signed))
        })
        .await?;
    }
    Ok(discarded)
}

/// Takes the frames of the replica `stream` of `scope` from `at`, where it
/// stands here, to `next`, where the hub said it stands: the stream's
/// epochs and gaps first, under the hub's word.
#[allow(clippy::too_many_arguments)]
async fn pull_stream(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    scope: &str,
    stream: &str,
    holding: i64,
    mut at: u64,
    next: u64,
    counted: Option<&super::standing::Counted<'_>>,
    source: Option<&fastctx_types::MachineName>,
) -> Result<(), String> {
    let library = &assets.library;
    let stream = stream.to_string();
    if at >= next {
        return Ok(());
    }
    let from = at;
    succession(assets, hub, link, key, scope, &stream).await?;
    // With a hub that keeps asset lines, frames come on one, each whole and
    // many to an exchange (`lines`).
    let lines = link.keeps_asset_lines();
    let turn = if lines { super::lines::RECORDS_A_TURN } else { RECORDS_A_TURN };
    while at < next {
        // Paused meanwhile, no more of it is asked for (`copies`), and it is
        // not taken as caught up.
        if assets.copies.paused_now().contains(scope) {
            return Err(format!("{PAUSED}{scope}"));
        }
        // Taken from the machine that orders it (`direct`), what took the
        // stream nowhere from here lately is not asked for again yet: the
        // same frames would come, and go nowhere again.
        if source.is_some() && assets.direct.stalled(scope, &stream, at) {
            return Err(format!("{}{stream} of {scope}: what its machine gave from {at} took it no further lately", super::direct::UNREACHED));
        }
        let frames = if let Some(machine) = source {
            // Frames that took it nowhere from here before come one at a
            // time when asked for again, not a line's worth to no end.
            let most = if assets.direct.stalled_before(scope, &stream, at) { 1 } else { fastctx_types::asset_lines::FRAMES };
            super::direct::frames(assets, hub, link, machine, scope, &stream, at, most).await?
        } else if lines {
            super::lines::frames(assets, link, scope, &stream, at).await?
        } else {
            let request = Request::Frames {
                scope: scope.to_string(),
                stream: stream.clone(),
                from: at,
                limit: FRAMES_AT_ONCE as u32,
            };
            let Reply::Frames { mut frames, ticket } = ask(assets, link, request).await? else {
                return Err("the hub did not give frames".into());
            };
            super::apart::bring(link, ticket, &mut frames).await?;
            frames
        };
        if frames.is_empty() {
            // The machine that keeps it said it stands further than it gave.
            if source.is_some() {
                assets.direct.stall(scope, &stream, at);
                return Err(format!("{}{stream} of {scope}: its machine gave nothing from {at} of {next}", super::direct::UNREACHED));
            }
            break;
        }
        let taken = if source.is_some() { "peer.frames" } else { "hub.frames" };
        // Frames that take it nowhere are not asked for again and again.
        let began = at;
        let mut went = false;
        for frame in frames {
            let bytes = (frame.header.0.len() + frame.signature.0.len() + frame.body.0.len()) as u64;
            fastctx_platform::readings::counter_of("sync.taken", taken).add(bytes);
            // A frame is put where its header says: one a machine gives is
            // of the very stream asked for, or nothing more of the batch is
            // kept.
            if source.is_some()
                && !Header::decode(&frame.header.0).is_ok_and(|header| header.scope == scope && header.stream == stream)
            {
                assets.direct.stall(scope, &stream, at);
                return Err(format!("{stream} of {scope}: its machine gave a frame of another stream"));
            }
            let raw = RawFrame {
                header: frame.header.0,
                signature: frame.signature.0,
                body: frame.body.0,
                objects: Vec::new(),
            };
            let scope_text = scope.to_string();
            let imported = blocking(library, move |library| {
                library.import(&scope_text, raw, &DeviceKeys, Holder::Replica, holding)
            })
            .await;
            // What its machine gave that cannot be kept is not asked for
            // again and again either.
            let imported = match imported {
                Ok(imported) => imported,
                Err(why) => {
                    if source.is_some() {
                        assets.direct.stall(scope, &stream, at);
                    }
                    return Err(why);
                }
            };
            match imported {
                Imported::Applied { next: reached, .. } => {
                    fastctx_platform::lab::die_at("import", &stream);
                    at = reached;
                    went = true;
                    super::standing::took(assets, hub, counted, bytes);
                    fastctx_platform::readings::counter_of("sync.applied", taken).add(bytes);
                    if stream == <fastctx_content::v2::ConversationMeta as fastctx_content::v2::Kind>::STREAM {
                        assets.metadata_moved.send_modify(|count| *count = count.wrapping_add(1));
                    }
                }
                Imported::Already => {}
                Imported::Gap { from, .. } => {
                    went |= from != began;
                    at = from;
                }
                Imported::Conflict => {
                    if source.is_some() {
                        assets.direct.stall(scope, &stream, at);
                    }
                    return Err(format!("{stream} of {scope} holds other records at a place the hub gave"));
                }
                Imported::Epoch(epoch) => {
                    if source.is_some() {
                        assets.direct.stall(scope, &stream, at);
                    }
                    return Err(format!(
                        "{stream} of {scope} has a frame of epoch {epoch} the hub gave no descriptor of"
                    ));
                }
            }
        }
        if !went {
            if source.is_some() {
                assets.direct.stall(scope, &stream, at);
            }
            return Err(format!("{stream} of {scope}: the frames given from {began} take it no further"));
        }
        if source.is_some() {
            assets.direct.went_on(scope, &stream);
        }
        // Its turn is over: the rest comes the next pass, at once.
        if at < next && at.saturating_sub(from) >= turn {
            assets.more.lock().expect("more").insert(hub.clone());
            return Err(format!("{GOES_ON}{stream} of {scope} at {at} of {next}"));
        }
    }
    Ok(())
}

/// Gives the hub again the frames of the replica `stream` of `scope` from
/// `from`, as they came from it before it recovered (design §17): each is
/// its producer's, signed, and the hub takes what still follows.
#[allow(clippy::too_many_arguments)]
async fn resupply(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    session: u64,
    scope: &str,
    stream: &str,
    mut from: u64,
) -> Result<(), String> {
    let library = &assets.library;
    loop {
        let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
        let frames =
            blocking(library, move |library| library.export(&scope_text, &stream_text, from, FRAMES_AT_ONCE)).await?;
        if frames.is_empty() {
            return Ok(());
        }
        for frame in frames {
            let header = Header::decode(&frame.header).map_err(|error| error.to_string())?;
            let frame = wire::Frame::whole(frame.header, frame.signature, frame.body);
            let mut proven = Vec::new();
            let (mut rounds, mut staged) = (0, 0);
            let received = loop {
                let Reply::Received { received } =
                    super::apart::take(assets, link, scope, &frame, &proven, &mut staged).await?
                else {
                    return Err("the hub did not take a frame given back".into());
                };
                let (objects, prove) = match &received {
                    wire::Received::Missing { objects } => (objects, false),
                    wire::Received::Prove { objects } => (objects, true),
                    _ => break received,
                };
                rounds += 1;
                if rounds > OBJECT_ROUNDS {
                    return Err(format!("{stream} of {scope}: the hub still wants objects of a frame given back"));
                }
                proven.extend(super::objects::put(library, link, scope, objects, prove).await?);
            };
            // The hub's word on what it took back is kept like any: what the
            // device brings if the hub is put back again (design §17).
            let receipt = |said: Said, word: Bytes| Receipt {
                stream: stream.to_string(),
                epoch: header.epoch,
                first: header.first,
                digest: header.digest,
                said,
                signature: word.0,
            };
            match received {
                wire::Received::Applied { change, prefix, word, .. } => {
                    let said = Said::Applied { change, prefix: array(&prefix, "a receipt's prefix")? };
                    keep(assets, hub, key, session, scope, receipt(said, word)).await?;
                    from = header.first + u64::from(header.count);
                }
                wire::Received::Parked { sequence, accumulated, word } => {
                    let said = Said::Parked { sequence, accumulated: array(&accumulated, "a receipt's accumulator")? };
                    keep(assets, hub, key, session, scope, receipt(said, word)).await?;
                    from = header.first + u64::from(header.count);
                }
                wire::Received::Gap { from: wanted, .. } if wanted > from => {
                    from = wanted;
                    break;
                }
                // What the hub will not take of a replica's is not this
                // machine's to settle: the stream's producer settles it.
                _ => return Ok(()),
            }
        }
    }
}

/// Where each stream of `scope` stands, checked with the hub; a replica's
/// that differs is cut back to the last place they share (design §7.5).
/// This machine's own streams are where the hub follows it: uploads and
/// refusals settle those, never a cut. Whether any was cut back.
async fn cursors(
    assets: &Assets,
    link: &Arc<Link>,
    scope: &str,
    holding: i64,
    ahead: &std::collections::BTreeSet<String>,
) -> Result<bool, String> {
    let library = &assets.library;
    let (scope_text, device) = (scope.to_string(), assets.device());
    let standing: Vec<(String, u64, [u8; 32])> =
        blocking(library, move |library| library.replica_standing(&scope_text, &device))
            .await?
            .into_iter()
            // A stream kept by the machine that orders it stands where that
            // machine has it, not where the hub does (`direct`).
            .filter(|(stream, _, _)| !ahead.contains(stream) && !super::direct::stays(link, scope, stream))
            .collect();
    if standing.is_empty() {
        return Ok(false);
    }
    let streams = standing
        .into_iter()
        .map(|(stream, sequence, head)| wire::Cursor { stream, sequence, head: head.into() })
        .collect();
    let Reply::Standing { streams } =
        ask(assets, link, Request::Cursors { scope: scope.to_string(), holding, streams }).await?
    else {
        return Err("the hub did not check the cursors".into());
    };
    let mut cut = false;
    for standing in streams.into_iter().filter(|standing| !standing.agrees) {
        let ends: Vec<(u64, [u8; 32])> =
            standing.ends.iter().filter_map(|(end, head)| head.array().map(|head| (*end, head))).collect();
        // The hub holds nothing of it: the hub lags, nothing here is cut.
        if ends.is_empty() {
            continue;
        }
        let (scope_text, stream_text) = (scope.to_string(), standing.stream.clone());
        let shared = blocking(library, move |library| library.shared_end(&scope_text, &stream_text, &ends)).await?;
        // What differs is kept aside, whole, before the stream is cut back:
        // it may be the last copy of what the hub signed before it was put
        // back (codex-795fd27c #1); rows no frame holds go with the cut.
        let (scope_text, stream_text) = (scope.to_string(), standing.stream);
        blocking(library, move |library| {
            let from = shared.map_or(0, |through| through + 1);
            if library.set_aside(&scope_text, &stream_text, from, "differs from the hub's history past where both agree")? > from {
                library.truncate(&scope_text, &stream_text, shared)?;
            }
            Ok(())
        })
        .await?;
        cut = true;
    }
    Ok(cut)
}

/// A scope's descriptor as far as its owner library goes.
#[derive(serde::Deserialize)]
struct OwnerOf {
    owner: String,
}

/// Facts already checked under the pinned key on the way in.
pub(super) struct Trusted;

impl fastctx_content::v2::HubVerifier for Trusted {
    fn verify(&self, _: &[u8], _: &[u8]) -> bool {
        true
    }
}

/// Checks a frame's signature against its ordering device's key.
pub(super) struct DeviceKeys;

impl fastctx_content::v2::Verifier for DeviceKeys {
    fn verify(&self, header: &Header, header_bytes: &[u8], signature: &[u8]) -> bool {
        let Ok(signature) = <[u8; 64]>::try_from(signature) else { return false };
        PublicKey::from_bytes(header.key)
            .verify(
                fastctx_net::sign::Domain::SyncSourceBatch,
                header_bytes,
                &fastctx_net::sign::Signature::from_bytes(signature),
            )
            .is_ok()
    }
}
