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
use fastctx_types::assets::{self as wire, Bytes, Readers, Reply, Request, StreamClass, VERSION, checkpoint_statement};
use std::sync::Arc;
use std::time::Duration;

/// How often a driver looks again when nothing woke it: what changed on the
/// hub is pushed, what is committed here wakes it, so this only catches a
/// push lost on the way (design §28.1).
const AGAIN: Duration = Duration::from_secs(30);
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
/// What a stream not yet caught up when its turn ended says: not a failure.
const GOES_ON: &str = "goes on next pass: ";
/// Receipts reported in one page.
const REPORTED_AT_ONCE: usize = 256;
/// How many times a frame's objects go before the hub must take it: put,
/// then proved where the put was not all of it.
const OBJECT_ROUNDS: usize = 3;

/// What a device proposes a scope it begins be described as.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Proposal {
    /// The hub it is to be described by: one hub's scope goes to no other.
    pub(crate) hub: String,
    pub(crate) readers: Readers,
    pub(crate) classes: Vec<StreamClass>,
}

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
        wire::Push::Moved { .. } | wire::Push::Fact { .. } => wake(node, hub),
        wire::Push::Revoked { scope } => {
            let Some(assets) = node.assets.as_ref() else { return };
            assets.forget_listing(hub);
            let (library, log, device) = (assets.library.clone(), node.log.clone(), assets.device());
            let (weak, hub) = (Arc::downgrade(node), hub.clone());
            fastctx_platform::task::spawn(async move {
                let key = scope.clone();
                if let Err(error) = blocking(&library, move |library| library.revoke_keeping(&key, &device)).await {
                    log.line(format_args!("ending what this machine held of {scope}: {error}"));
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
            Ok(Some((link, network))) if link.view().asset_sync == VERSION => {
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
const REVOKED: &str = "no longer read here: ";

/// Asks the hub `request` of a scope; where the hub says this machine reads
/// the scope no more, its holding here ends (design §4.5, §6.3).
pub(super) async fn ask(assets: &Assets, link: &Arc<Link>, request: Request) -> Result<Reply, String> {
    match link.assets(request).await? {
        Reply::Revoked { scope } => {
            // What may be given back after it is found by listing all again:
            // its description is older than any listing goes on from (BB13).
            assets.forget_listing(&link.membership().id);
            let key = scope.clone();
            let device = assets.device();
            blocking(&assets.library, move |library| library.revoke_keeping(&key, &device)).await?;
            // And again once the holding ended (DD01).
            assets.forget_listing(&link.membership().id);
            Err(format!("{REVOKED}{scope}"))
        }
        reply => Ok(reply),
    }
}

/// Runs `work` on the library off the async threads.
pub(super) async fn blocking<T: Send + 'static>(
    library: &Library,
    work: impl FnOnce(&Library) -> fastctx_content::Result<T> + Send + 'static,
) -> Result<T, String> {
    let library = library.clone();
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
    let key = open(assets, hub, link, session).await?;
    let mut failed = describe(assets, hub, link, &key, network).await.err();
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
        match discover(assets, hub, link, &key, network).await {
            Ok(whole) => listed = whole,
            Err(why) => {
                failed.get_or_insert(format!("listing what this machine may hold: {why}"));
            }
        }
    }
    // This hub's scopes alone: another hub's go to no other.
    let theirs = hub.as_str().to_string();
    let held = blocking(library, move |library| library.live_scopes_of(&theirs)).await?;
    // Not pulled whole in its holding period yet — installed, held anew,
    // described otherwise, or its pull cut short — a scope is looked at by
    // itself until it is: no change number after this machine's need show
    // its streams' past (design §28.1, E01).
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
        let fresh = unpulled.contains(&scope);
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
    let marks = if isolated.is_none() && !fresh.is_empty() { tops(assets, link, &fresh).await.ok() } else { None };
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
    // A scope that fails holds up none after it; the pass reports the first.
    for (scope, holding, first) in &scopes {
        // Each step goes on whatever the one before met; a revoked scope
        // goes no further.
        let uploaded = upload(assets, hub, link, &key, session, scope).await;
        if uploaded.is_ok() {
            assets.uploads.lock().expect("uploads").remove(scope);
        }
        let pulled = match &uploaded {
            Err(why) if why.starts_with(REVOKED) => Err(why.clone()),
            _ => pull(assets, hub, link, &key, session, scope, *holding, isolated.is_some()).await,
        };
        // What the hub lacks of this machine's own streams, found as they
        // were pulled — the hub put back (§17) — goes up again this very
        // pass, from where the hub is.
        let uploaded = if uploaded.is_ok() && assets.uploads.lock().expect("uploads").contains(scope) {
            let again = upload(assets, hub, link, &key, session, scope).await;
            if again.is_ok() {
                assets.uploads.lock().expect("uploads").remove(scope);
            }
            again
        } else {
            uploaded
        };
        // Isolated, what the hub lacks of what is here still goes back to it
        // (AA04); nothing comes down.
        if isolated.is_some() {
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
            continue;
        }
        // Where each stream stands is checked whatever a stream's frames
        // met: a frame that does not chain to what is here is what checking
        // finds the shared place for.
        let checked = match &pulled {
            Err(why) if why.starts_with(REVOKED) => Err(why.clone()),
            Err(_) => Ok(false),
            Ok((ahead, _)) => cursors(assets, link, scope, *holding, ahead).await,
        };
        // What checking cut back comes again from the hub this very pass
        // (HH05).
        let (pulled, checked) = match checked {
            Ok(true) => (pull(assets, hub, link, &key, session, scope, *holding, false).await, Ok(())),
            Ok(false) => (pulled, Ok(())),
            Err(why) => (pulled, Err(why)),
        };
        // Holding what the hub has of them, the streams to be ordered here
        // are taken over (design §8).
        let checked = match (&pulled, checked) {
            (Ok((_, None)), Ok(())) => super::takeover::run(node, assets, hub, link, &key, scope).await,
            (_, checked) => checked,
        };
        // Every stream pulled and checked: the scope is pulled whole in its
        // holding period, and the change numbers carry it from now on.
        if *first && !untold && matches!(&pulled, Ok((_, None))) && checked.is_ok() {
            let (scope, holding) = (scope.clone(), *holding);
            if let Err(why) = blocking(library, move |library| library.pulled(&scope, holding)).await {
                failed.get_or_insert(why);
            }
        }
        let pulled = pulled.and_then(|(_, failed)| failed.map_or(Ok(()), Err));
        let mut whole_here = true;
        for done in [uploaded, pulled, checked] {
            if let Err(why) = done
                && !why.starts_with(REVOKED)
            {
                whole_here = false;
                failed.get_or_insert(format!("{scope}: {why}"));
            }
        }
        // Not gone through whole, it is looked at by itself again next pass
        // until it is (HH03, HH06), and its owner library is not taken as
        // reached.
        if !whole_here && let Some(owner) = owner_of.get(scope) {
            short.insert(owner.clone());
        }
        let mut relook = assets.relook.lock().expect("relook");
        if whole_here {
            relook.remove(scope);
        } else {
            relook.insert(scope.clone());
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
            if let Err(why) = feed(assets, hub, link, &key, owner, &unpulled).await {
                failed.get_or_insert(format!("what changed in {owner}: {why}"));
            }
        }
    }
    // What ended holding periods left goes a pass at a time.
    if let Err(why) = blocking(library, |library| library.purge()).await {
        failed.get_or_insert(format!("purging what ended: {why}"));
    }
    if failed.is_none()
        && whole
        && let Some(revision) = waited
        && waiting_revision(link) == Some(revision)
        && let Err(why) = cleared(node, assets, hub, link, revision).await
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
    for failed in super::objects::fetch(assets, hub, link).await {
        node.log.line(format_args!("assets with {hub}: {failed}; asked again later"));
    }
    // Where this machine stands goes to the hub as it changes (design §28.9).
    if let Err(why) = super::standing::report(assets, hub, link).await {
        node.log.line(format_args!("assets with {hub}: telling where this machine stands: {why}"));
    }
    failed.map_or(Ok(()), Err)
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
    crate::replicas::publication::prune(node).await?;
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
    // Said only if, after all that, still nothing of another network's is
    // here and no other network came meanwhile.
    if elsewhere(assets, hub).await?.is_some() || waiting_revision(link) != Some(revision) {
        return Ok(());
    }
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
/// the next pass. The first failure, the rest gone on with.
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
            if changed.epoch > 0 && known.is_none_or(|known| known < changed.epoch) {
                succession(assets, hub, link, key, &changed.scope, &changed.stream).await?;
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
            pull_stream(assets, hub, link, key, &changed.scope, &changed.stream, holding, at, changed.next, Some(counted)).await
        }
        .await;
        if let Err(why) = done
            && !why.starts_with(REVOKED)
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
        version: VERSION,
        seen: assets.seen(hub),
        reported: wired,
        pinned: assets.pinned(hub),
        lineage: assets.lineage(hub),
    };
    let (offered, checkpointed, rotations, closed) = match link.assets(request).await? {
        Reply::Ready { key, checkpointed, rotations, closed, .. } => (key, checkpointed, rotations, closed),
        Reply::Sealed { why } => return Err(format!("the hub keeps every asset sealed: {why}")),
        _ => return Err("the hub did not open the session".into()),
    };
    let key = assets.pin_following(hub, &offered, &rotations)?;
    if closed {
        // The hub set the history this machine saw aside when it recovered:
        // its head is forgotten and its receipts wait no more; what the hub
        // lacks is given back as each scope is gone through (`pull`), in a
        // whole look this very pass.
        assets.looked.lock().expect("looked").remove(hub);
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
        let readers = Readers::Account { account };
        let scope = format!("{}records", readers.key_prefix(hub.as_str()));
        let proposal = Proposal { hub: hub.to_string(), readers, classes: vec![StreamClass { prefix: String::new(), holders: wire::Holders::Readers }] };
        let (network, proposal) = (network.clone(), serde_json::to_vec(&proposal).map_err(|error| error.to_string())?);
        blocking(library, move |library| {
            if library.holding(&scope)?.is_none() {
                library.propose_on(&network, &scope, proposal)?;
            }
            Ok::<_, fastctx_content::Error>(())
        })
        .await?;
    }
    // And each project this machine keeps the bodies of: the hub writes its
    // ledger there (design §28.18).
    let view = link.view();
    let projects: Vec<fastctx_types::access::TownId> = view
        .directory
        .projects
        .iter()
        .filter(|project| view.directory.member_keeps_project(&view.me, &project.town))
        .map(|project| project.town.clone())
        .collect();
    drop(view);
    for town in projects {
        let readers = Readers::Project { town };
        let scope = format!("{}ledger", readers.key_prefix(hub.as_str()));
        let proposal = Proposal { hub: hub.to_string(), readers, classes: vec![StreamClass { prefix: String::new(), holders: wire::Holders::Readers }] };
        let (network, proposal) = (network.clone(), serde_json::to_vec(&proposal).map_err(|error| error.to_string())?);
        blocking(library, move |library| {
            if library.holding(&scope)?.is_none() {
                library.propose_on(&network, &scope, proposal)?;
            }
            Ok::<_, fastctx_content::Error>(())
        })
        .await?;
    }
    for (scope, proposal) in blocking(library, |library| library.provisionals()).await? {
        let Some(proposal) = proposal else { continue };
        let Proposal { hub: meant, readers, classes } =
            serde_json::from_slice(&proposal).map_err(|error| error.to_string())?;
        if meant != hub.as_str() {
            continue;
        }
        let described = link.assets(Request::Describe { scope: scope.clone(), readers, classes }).await;
        // A project's or an account's scope the hub does not describe for
        // this machine — no account here reads it any more — holds up no
        // conversation's.
        if described.is_err() && (scope.starts_with("p/") || scope.starts_with("a/")) {
            continue;
        }
        let Reply::Described { descriptor } = described? else {
            return Err("the hub did not describe the scope".into());
        };
        if !assets.heard(hub, key, &descriptor.fact.0, &descriptor.word.0) {
            return Err(format!("the descriptor of {scope} is not the hub's"));
        }
        let read: wire::Descriptor = serde_json::from_slice(&descriptor.fact.0).map_err(|error| error.to_string())?;
        if read.scope != scope || read.hub != hub.as_str() {
            return Err(format!("the hub described another scope than {scope}"));
        }
        let network = network.clone();
        blocking(library, move |library| library.install_on(&network, &scope, descriptor.fact.0, read.seq as i64))
            .await?;
    }
    Ok(())
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
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    network: &Network,
) -> Result<Option<std::collections::BTreeSet<String>>, String> {
    let library = &assets.library;
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
            let (scope, fact) = (held.scope.clone(), held.descriptor.fact.0);
            let here = blocking(library, move |library| library.held_listing(&scope)).await?;
            let (described, allowed) = here.unzip();
            if described.as_deref() != Some(fact.as_slice()) {
                let (network, scope, revision) = (network.clone(), held.scope.clone(), read.seq as i64);
                blocking(library, move |library| library.install_on(&network, &scope, fact, revision)).await?;
            }
            // What it may hold of the scope changed with nothing else of it:
            // pulled whole again, its streams each judged anew (HH04).
            let holds = held.holds.join("\n");
            if allowed.flatten().as_deref() != Some(holds.as_str()) {
                let scope = held.scope.clone();
                blocking(library, move |library| library.allow(&scope, &holds)).await?;
            }
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
    // A stream that fails holds up none after it; the scope reports the first.
    let mut failed: Option<String> = waiting_failed;
    for (stream, from) in blocking(library, move |library| library.uploads(&key_text, &device)).await? {
        let done: Result<(), String> = async {
            let mut from = from;
            loop {
                let (scope_text, stream_text) = (scope.to_string(), stream.clone());
                let frames =
                    blocking(library, move |library| library.export(&scope_text, &stream_text, from, FRAMES_AT_ONCE))
                        .await?;
                if frames.is_empty() {
                    break;
                }
                for frame in frames {
                    let header = Header::decode(&frame.header).map_err(|error| error.to_string())?;
                    let frame = wire::Frame::whole(frame.header, frame.signature, frame.body);
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
                    let receipt = |said: Said, word: Bytes| Receipt {
                        stream: stream.clone(),
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
                        }
                        wire::Received::Parked { sequence, accumulated, word } => {
                            let said =
                                Said::Parked { sequence, accumulated: array(&accumulated, "a receipt's accumulator")? };
                            keep(assets, hub, key, session, scope, receipt(said, word)).await?;
                        }
                        wire::Received::Rejected { from: cut, word, .. } => {
                            keep(assets, hub, key, session, scope, receipt(Said::Rejected { from: cut }, word)).await?;
                            refused(assets, hub, link, key, scope, &stream, cut).await?;
                            return Ok(());
                        }
                        wire::Received::Gap { from: wanted, .. } => {
                            from = wanted;
                            continue;
                        }
                        wire::Received::Missing { .. } | wire::Received::Prove { .. } => {
                            return Err(format!("{stream} of {scope}: the hub still wants objects after they went"));
                        }
                    }
                    from = header.first + u64::from(header.count);
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
    failed.map_or(Ok(()), Err)
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
    blocking(library, move |library| library.new_epoch(&scope_text, &stream_text, fact.epoch, descriptor.fact.0)).await
}

/// Commits what waits of `stream` again, in its current epoch.
async fn resubmit(assets: &Assets, scope: &str, stream: &str) -> Result<(), String> {
    loop {
        let (scope_text, stream_text, signer) = (scope.to_string(), stream.to_string(), assets.signer());
        let committed =
            blocking(&assets.library, move |library| library.resubmit(&scope_text, &stream_text, 64, signer)).await?;
        if committed == 0 {
            return Ok(());
        }
    }
}

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
    for stream in
        withheld.into_iter().filter(|stream| local.get(stream).is_some_and(|at| *at > 0) || !already.contains(stream))
    {
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
    let local = if discard_unaccepted(assets, hub, link, key, scope, &theirs).await? {
        let scope_text = scope.to_string();
        blocking(library, move |library| library.streams(&scope_text)).await?.into_iter().collect()
    } else {
        local
    };
    let (scope_text, device) = (scope.to_string(), assets.device());
    let own: std::collections::BTreeSet<String> =
        blocking(library, move |library| library.own_streams(&scope_text, &device)).await?.into_iter().collect();
    for (stream, at) in &local {
        let hub_at = theirs.get(stream).copied();
        if own.contains(stream) {
            let hub_at = hub_at.unwrap_or(0);
            if *at > hub_at {
                let (scope_text, stream_text) = (scope.to_string(), stream.clone());
                blocking(library, move |library| library.rewind_uploads(&scope_text, &stream_text, hub_at)).await?;
                // Gone up again from where the hub is, this very pass
                // (`pass`), and until it has.
                assets.uploads.lock().expect("uploads").insert(scope.to_string());
            }
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
    for wire::StreamAt { stream, next, .. } in streams {
        let at = local.get(&stream).copied().unwrap_or(0);
        if let Err(why) = pull_stream(assets, hub, link, key, scope, &stream, holding, at, next, None).await {
            if why.starts_with(REVOKED) {
                return Err(why);
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
        // The hub has none of it: this machine's to send up.
        if theirs.get(&stream).copied().unwrap_or(0) == 0 {
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
        discarded |=
            blocking(library, move |library| library.discard_unaccepted(&scope_text, &stream_text, &device, Some(&checkpoint))).await?;
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
) -> Result<(), String> {
    let library = &assets.library;
    let stream = stream.to_string();
    if at >= next {
        return Ok(());
    }
    let from = at;
    succession(assets, hub, link, key, scope, &stream).await?;
    while at < next {
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
        if frames.is_empty() {
            break;
        }
        // Frames that take it nowhere are not asked for again and again.
        let began = at;
        let mut went = false;
        for frame in frames {
            let bytes = (frame.header.0.len() + frame.signature.0.len() + frame.body.0.len()) as u64;
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
            .await?;
            match imported {
                Imported::Applied { next: reached, .. } => {
                    at = reached;
                    went = true;
                    super::standing::took(assets, hub, counted, bytes);
                }
                Imported::Already => {}
                Imported::Gap { from, .. } => {
                    went |= from != began;
                    at = from;
                }
                Imported::Conflict => {
                    return Err(format!("{stream} of {scope} holds other records at a place the hub gave"));
                }
                Imported::Epoch(epoch) => {
                    return Err(format!(
                        "{stream} of {scope} has a frame of epoch {epoch} the hub gave no descriptor of"
                    ));
                }
            }
        }
        if !went {
            return Err(format!("{stream} of {scope}: the hub's frames from {began} take it no further"));
        }
        // Its turn is over: the rest comes the next pass, at once.
        if at < next && at.saturating_sub(from) >= RECORDS_A_TURN {
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
            .filter(|(stream, _, _)| !ahead.contains(stream))
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
        let (scope_text, stream_text) = (scope.to_string(), standing.stream);
        blocking(library, move |library| library.truncate(&scope_text, &stream_text, shared)).await?;
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
