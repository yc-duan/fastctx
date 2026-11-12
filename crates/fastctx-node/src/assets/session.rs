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
use fastctx_content::v2::{EpochFact, Header, Holder, Imported, Library, RawFrame, Receipt, Said};
use fastctx_net::link::Link;
use fastctx_net::sign::PublicKey;
use fastctx_types::access::HubId;
use fastctx_types::assets::{self as wire, Bytes, Readers, Reply, Request, StreamClass, VERSION, checkpoint_statement};
use std::sync::Arc;
use std::time::Duration;

/// How often a driver looks again when nothing woke it.
const AGAIN: Duration = Duration::from_secs(5);
/// Frames uploaded or taken at once.
const FRAMES_AT_ONCE: usize = 16;
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
            let (library, log) = (assets.library.clone(), node.log.clone());
            fastctx_platform::task::spawn(async move {
                let key = scope.clone();
                if let Err(error) = blocking(&library, move |library| library.revoke(&key)).await {
                    log.line(format_args!("ending what this machine held of {scope}: {error}"));
                }
            });
        }
        wire::Push::Checkpointed { checkpoint } => {
            let Some(assets) = node.assets.as_ref() else { return };
            let Some((session, key)) = assets.current.lock().expect("sessions").get(hub).copied() else { return };
            let Some(head) = checkpoint.head.array() else { return };
            let statement = checkpoint_statement(&checkpoint.scope, checkpoint.change, checkpoint.parked, checkpoint.seq, &head);
            if !assets.heard(hub, &key, &statement, &checkpoint.word.0) {
                return;
            }
            let (library, log) = (assets.library.clone(), node.log.clone());
            fastctx_platform::task::spawn(async move {
                let scope = checkpoint.scope.clone();
                if let Err(error) =
                    blocking(&library, move |library| library.checkpointed(&checkpoint.scope, checkpoint.change, checkpoint.parked, session)).await
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
        let Some(link) = node.link_of(&hub) else {
            if let Some(assets) = node.assets.as_ref() {
                assets.drivers.lock().expect("drivers").remove(&hub);
            }
            return;
        };
        let speaks = link.view().asset_sync == VERSION;
        let failed = if speaks { pass(&node, &hub, &link).await.err() } else { None };
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
                AGAIN
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
async fn ask(assets: &Assets, link: &Arc<Link>, request: Request) -> Result<Reply, String> {
    match link.assets(request).await? {
        Reply::Revoked { scope } => {
            let key = scope.clone();
            blocking(&assets.library, move |library| library.revoke(&key)).await?;
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

/// One pass with the hub.
async fn pass(node: &Arc<Node>, hub: &HubId, link: &Arc<Link>) -> Result<(), String> {
    let assets = node.assets.as_ref().ok_or("the asset library is unavailable")?;
    let library = &assets.library;
    let session = assets.session();
    let key = open(assets, hub, link, session).await?;
    let mut failed = describe(assets, hub, link, &key).await.err();
    // This hub's scopes alone: another hub's go to no other.
    let scopes: Vec<(String, i64)> = blocking(library, |library| library.live_scopes())
        .await?
        .into_iter()
        .filter(|(_, descriptor, _)| {
            serde_json::from_slice::<wire::Descriptor>(descriptor).is_ok_and(|descriptor| descriptor.hub == hub.as_str())
        })
        .map(|(scope, _, holding)| (scope, holding))
        .collect();
    // A scope that fails holds up none after it; the pass reports the first.
    for (scope, holding) in &scopes {
        let done = async {
            upload(assets, hub, link, &key, session, scope).await?;
            pull(assets, hub, link, &key, scope, *holding).await?;
            cursors(assets, link, scope, *holding).await
        }
        .await;
        if let Err(why) = done
            && !why.starts_with(REVOKED)
        {
            failed.get_or_insert(format!("{scope}: {why}"));
        }
    }
    // What ended holding periods left goes a pass at a time.
    if let Err(why) = blocking(library, |library| library.purge()).await {
        failed.get_or_insert(format!("purging what ended: {why}"));
    }
    if failed.is_none()
        && let Err(why) = cleared(node, assets, hub, link).await
    {
        failed = Some(why);
    }
    for failed in super::objects::fetch(assets, hub, link).await {
        node.log.line(format_args!("assets with {hub}: {failed}; asked again later"));
    }
    failed.map_or(Ok(()), Err)
}

/// Shares of this machine waiting for it to clear (design §6.3): once a
/// whole pass found nothing it may no longer hold — each scope asked of
/// the hub as it judges with the waiting shares — and the records it may
/// no longer keep are pruned, it says so under its key, at the holding
/// revision the shares wait at.
async fn cleared(node: &Arc<Node>, assets: &Assets, hub: &HubId, link: &Arc<Link>) -> Result<(), String> {
    let view = link.view();
    let directory = &view.directory;
    let me = link.name();
    let me = directory.canonical(&me).clone();
    let Some(revision) = directory.shares.iter().filter(|share| share.machine == me).filter_map(|share| share.pending).max() else {
        return Ok(());
    };
    if assets.cleared.lock().expect("cleared").get(hub) == Some(&revision) {
        return Ok(());
    }
    if !crate::replicas::cleanup::started(node) {
        // The records' pruning has not begun yet: said at a later pass.
        return Ok(());
    }
    crate::replicas::publication::prune(node).await?;
    let proof = assets.sign_cleared(me.as_str(), revision);
    let Reply::Cleared { activated } = link.assets(Request::Cleared { revision, proof: proof.into() }).await? else {
        return Err("the hub did not take this machine's word that it cleared".into());
    };
    assets.cleared.lock().expect("cleared").insert(hub.clone(), revision);
    if activated > 0 {
        node.log.line(format_args!("cleared for {activated} share(s) of this machine; they are in force now"));
    }
    Ok(())
}

/// The scopes `hub` described here.
pub(super) async fn hub_scopes(library: &Library, hub: &HubId) -> Result<std::collections::BTreeSet<String>, String> {
    let hub = hub.as_str().to_string();
    Ok(blocking(library, |library| library.live_scopes())
        .await?
        .into_iter()
        .filter(|(_, descriptor, _)| serde_json::from_slice::<wire::Descriptor>(descriptor).is_ok_and(|descriptor| descriptor.hub == hub))
        .map(|(scope, _, _)| scope)
        .collect())
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
                Said::Parked { sequence, accumulated } => wire::Said::Parked { sequence: *sequence, accumulated: (*accumulated).into() },
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
    let request = Request::Open { version: VERSION, seen: assets.seen(hub), reported: wired, pinned: assets.pinned(hub) };
    let (offered, checkpointed, rotations, closed) = match link.assets(request).await? {
        Reply::Ready { key, checkpointed, rotations, closed, .. } => (key, checkpointed, rotations, closed),
        Reply::Sealed { why } => return Err(format!("the hub keeps every asset sealed: {why}")),
        _ => return Err("the hub did not open the session".into()),
    };
    let key = assets.pin_following(hub, &offered, &rotations)?;
    if closed {
        // The hub set the history this machine saw aside when it recovered:
        // its head is forgotten and its receipts wait no more; what the hub
        // lacks is given back as each scope is gone through (`pull`).
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
        let statement = checkpoint_statement(&checkpoint.scope, checkpoint.change, checkpoint.parked, checkpoint.seq, &head);
        if !assets.heard(hub, &key, &statement, &checkpoint.word.0) {
            continue;
        }
        blocking(library, move |library| library.checkpointed(&checkpoint.scope, checkpoint.change, checkpoint.parked, session)).await?;
    }
    Ok(key)
}

/// The scopes this machine began before any hub described them, described.
async fn describe(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey) -> Result<(), String> {
    let library = &assets.library;
    for (scope, proposal) in blocking(library, |library| library.provisionals()).await? {
        let Some(proposal) = proposal else { continue };
        let Proposal { hub: meant, readers, classes } = serde_json::from_slice(&proposal).map_err(|error| error.to_string())?;
        if meant != hub.as_str() {
            continue;
        }
        let Reply::Described { descriptor } = link.assets(Request::Describe { scope: scope.clone(), readers, classes }).await? else {
            return Err("the hub did not describe the scope".into());
        };
        if !assets.heard(hub, key, &descriptor.fact.0, &descriptor.word.0) {
            return Err(format!("the descriptor of {scope} is not the hub's"));
        }
        let read: wire::Descriptor = serde_json::from_slice(&descriptor.fact.0).map_err(|error| error.to_string())?;
        if read.scope != scope || read.hub != hub.as_str() {
            return Err(format!("the hub described another scope than {scope}"));
        }
        blocking(library, move |library| library.install(&scope, descriptor.fact.0, read.seq as i64)).await?;
    }
    Ok(())
}

/// The frames of this machine's own streams of `scope` past what the hub
/// has.
async fn upload(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, session: u64, scope: &str) -> Result<(), String> {
    let library = &assets.library;
    // What a refusal took out goes again first, the hub asked for a new
    // epoch where none was opened yet (a pass cut short after the refusal).
    let scope_text = scope.to_string();
    for (stream, cut, renewed) in blocking(library, move |library| library.waiting(&scope_text)).await? {
        if !renewed {
            renew(assets, hub, link, key, scope, &stream, cut).await?;
        }
        resubmit(assets, scope, &stream).await?;
    }
    let device = assets.device();
    let key_text = scope.to_string();
    // A stream that fails holds up none after it; the scope reports the first.
    let mut failed: Option<String> = None;
    for (stream, from) in blocking(library, move |library| library.uploads(&key_text, &device)).await? {
        let done: Result<(), String> = async {
        let mut from = from;
        loop {
            let (scope_text, stream_text) = (scope.to_string(), stream.clone());
            let frames = blocking(library, move |library| library.export(&scope_text, &stream_text, from, FRAMES_AT_ONCE)).await?;
            if frames.is_empty() {
                break;
            }
            for frame in frames {
                let header = Header::decode(&frame.header).map_err(|error| error.to_string())?;
                let frame = wire::Frame { header: frame.header.into(), signature: frame.signature.into(), body: frame.body.into() };
                // Objects the hub has not, or wants proved, go first; then
                // the frame again, naming what this machine proved.
                let mut proven: Vec<Bytes> = Vec::new();
                let mut rounds = 0;
                let received = loop {
                    let request = Request::Take { scope: scope.to_string(), frame: frame.clone(), proven: proven.clone() };
                    let Reply::Received { received } = ask(assets, link, request).await? else {
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
                        let said = Said::Parked { sequence, accumulated: array(&accumulated, "a receipt's accumulator")? };
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
async fn keep(assets: &Assets, hub: &HubId, key: &PublicKey, session: u64, scope: &str, receipt: Receipt) -> Result<(), String> {
    let frame = fastctx_content::v2::FrameKey {
        scope,
        stream: &receipt.stream,
        epoch: receipt.epoch,
        first: receipt.first,
        digest: &receipt.digest,
    };
    let statement = match &receipt.said {
        Said::Applied { change, prefix } => Some(fastctx_content::v2::applied_statement(&frame, *change, prefix)),
        Said::Parked { sequence, accumulated } => Some(fastctx_content::v2::parked_statement(&frame, *sequence, accumulated)),
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
async fn refused(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, stream: &str, cut: u64) -> Result<(), String> {
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    blocking(&assets.library, move |library| library.rejected(&scope_text, &stream_text, cut)).await?;
    renew(assets, hub, link, key, scope, stream, cut).await?;
    resubmit(assets, scope, stream).await
}

/// The hub opens a new epoch of `stream` for this machine at `cut`, where
/// the stream was cut back to: what was committed into it since goes with
/// what the refusal took out.
async fn renew(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, stream: &str, cut: u64) -> Result<(), String> {
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
        let committed = blocking(&assets.library, move |library| library.resubmit(&scope_text, &stream_text, 64, signer)).await?;
        if committed == 0 {
            return Ok(());
        }
    }
}

/// The frames of the streams of `scope` this machine holds, past what it
/// has, under the hub's signed epochs and gap facts.
async fn pull(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, holding: i64) -> Result<(), String> {
    let library = &assets.library;
    let Reply::Streams { streams } = ask(assets, link, Request::Streams { scope: scope.to_string() }).await? else {
        return Err("the hub did not list the scope's streams".into());
    };
    let scope_text = scope.to_string();
    let local: std::collections::BTreeMap<String, u64> =
        blocking(library, move |library| library.streams(&scope_text)).await?.into_iter().collect();
    // What the hub lacks that this machine has — the hub recovered to
    // before it (design §17) — goes back: its own streams' uploading goes on
    // from where the hub is, a replica's frames are given again.
    let theirs: std::collections::BTreeMap<String, u64> = streams.iter().map(|at| (at.stream.clone(), at.next)).collect();
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
            }
        } else if let Some(hub_at) = hub_at
            && *at > hub_at
        {
            resupply(assets, link, scope, stream, hub_at).await?;
        }
    }
    let mut failed: Option<String> = None;
    for wire::StreamAt { stream, next } in streams {
        let done: Result<(), String> = async {
        let mut at = local.get(&stream).copied().unwrap_or(0);
        if at >= next {
            return Ok(());
        }
        let Reply::Succession { epochs, gaps } =
            ask(assets, link, Request::Succession { scope: scope.to_string(), stream: stream.clone() }).await?
        else {
            return Err("the hub did not give the stream's succession".into());
        };
        for fact in epochs {
            let signed = fastctx_content::v2::Signed { fact: fact.fact.0, signature: fact.word.0 };
            let scope_text = scope.to_string();
            let verifier = Pinning { assets, hub, key: *key };
            // The verifier borrows this machine's state: checked here, kept on the library's thread.
            if !fastctx_content::v2::HubVerifier::verify(&verifier, &signed.fact, &signed.signature) {
                return Err("an epoch's descriptor is not the hub's".into());
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
        while at < next {
            let request = Request::Frames { scope: scope.to_string(), stream: stream.clone(), from: at, limit: FRAMES_AT_ONCE as u32 };
            let Reply::Frames { frames } = ask(assets, link, request).await? else {
                return Err("the hub did not give frames".into());
            };
            if frames.is_empty() {
                break;
            }
            for frame in frames {
                let raw = RawFrame { header: frame.header.0, signature: frame.signature.0, body: frame.body.0, objects: Vec::new() };
                let scope_text = scope.to_string();
                let imported =
                    blocking(library, move |library| library.import(&scope_text, raw, &DeviceKeys, Holder::Replica, holding)).await?;
                match imported {
                    Imported::Applied { next: reached, .. } => at = reached,
                    Imported::Already => {}
                    Imported::Gap { from, .. } => at = from,
                    Imported::Conflict => return Err(format!("{stream} of {scope} holds other records at a place the hub gave")),
                    Imported::Epoch(epoch) => return Err(format!("{stream} of {scope} has a frame of epoch {epoch} the hub gave no descriptor of")),
                }
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

/// Gives the hub again the frames of the replica `stream` of `scope` from
/// `from`, as they came from it before it recovered (design §17): each is
/// its producer's, signed, and the hub takes what still follows.
async fn resupply(assets: &Assets, link: &Arc<Link>, scope: &str, stream: &str, mut from: u64) -> Result<(), String> {
    let library = &assets.library;
    loop {
        let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
        let frames = blocking(library, move |library| library.export(&scope_text, &stream_text, from, FRAMES_AT_ONCE)).await?;
        if frames.is_empty() {
            return Ok(());
        }
        for frame in frames {
            let header = Header::decode(&frame.header).map_err(|error| error.to_string())?;
            let frame = wire::Frame { header: frame.header.into(), signature: frame.signature.into(), body: frame.body.into() };
            let Reply::Received { received } = ask(assets, link, Request::Take { scope: scope.to_string(), frame, proven: Vec::new() }).await? else {
                return Err("the hub did not take a frame given back".into());
            };
            match received {
                wire::Received::Applied { .. } | wire::Received::Parked { .. } => from = header.first + u64::from(header.count),
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
/// refusals settle those, never a cut.
async fn cursors(assets: &Assets, link: &Arc<Link>, scope: &str, holding: i64) -> Result<(), String> {
    let library = &assets.library;
    let (scope_text, device) = (scope.to_string(), assets.device());
    let standing = blocking(library, move |library| library.replica_standing(&scope_text, &device)).await?;
    if standing.is_empty() {
        return Ok(());
    }
    let streams = standing
        .into_iter()
        .map(|(stream, sequence, head)| wire::Cursor { stream, sequence, head: head.into() })
        .collect();
    let Reply::Standing { streams } = ask(assets, link, Request::Cursors { scope: scope.to_string(), holding, streams }).await? else {
        return Err("the hub did not check the cursors".into());
    };
    for standing in streams.into_iter().filter(|standing| !standing.agrees) {
        let ends: Vec<(u64, [u8; 32])> =
            standing.ends.iter().filter_map(|(end, head)| head.array().map(|head| (*end, head))).collect();
        let (scope_text, stream_text) = (scope.to_string(), standing.stream.clone());
        let shared = blocking(library, move |library| library.shared_end(&scope_text, &stream_text, &ends)).await?;
        let (scope_text, stream_text) = (scope.to_string(), standing.stream);
        blocking(library, move |library| library.truncate(&scope_text, &stream_text, shared)).await?;
    }
    Ok(())
}

/// Facts already checked under the pinned key on the way in.
struct Trusted;

impl fastctx_content::v2::HubVerifier for Trusted {
    fn verify(&self, _: &[u8], _: &[u8]) -> bool {
        true
    }
}

/// Checks a frame's signature against its ordering device's key.
struct DeviceKeys;

impl fastctx_content::v2::Verifier for DeviceKeys {
    fn verify(&self, header: &Header, header_bytes: &[u8], signature: &[u8]) -> bool {
        let Ok(signature) = <[u8; 64]>::try_from(signature) else { return false };
        PublicKey::from_bytes(header.key)
            .verify(fastctx_net::sign::Domain::SyncSourceBatch, header_bytes, &fastctx_net::sign::Signature::from_bytes(signature))
            .is_ok()
    }
}
