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
    describe(assets, hub, link, &key).await?;
    let scopes = blocking(library, |library| library.live_scopes()).await?;
    for (scope, _, holding) in &scopes {
        upload(assets, hub, link, &key, session, scope).await?;
        pull(assets, hub, link, &key, scope, *holding).await?;
        cursors(library, link, scope, *holding).await?;
    }
    for failed in super::objects::fetch(assets, link).await {
        node.log.line(format_args!("assets with {hub}: {failed}; asked again later"));
    }
    Ok(())
}

/// Opens a session: what this machine saw of the hub's history goes first.
async fn open(assets: &Assets, hub: &HubId, link: &Arc<Link>, session: u64) -> Result<PublicKey, String> {
    let library = &assets.library;
    let mut reported = Vec::new();
    let mut after = None;
    loop {
        let mark = after.clone();
        let page = blocking(library, move |library| library.unwitnessed(mark.as_ref(), REPORTED_AT_ONCE)).await?;
        reported.extend(page.receipts);
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
    let reply = link.assets(Request::Open { version: VERSION, seen: assets.seen(hub), reported: wired }).await?;
    let (offered, checkpointed) = match reply {
        Reply::Ready { key, checkpointed, .. } => (key, checkpointed),
        Reply::Sealed { why } => return Err(format!("the hub keeps every asset sealed: {why}")),
        _ => return Err("the hub did not open the session".into()),
    };
    let key = assets.pin(hub, &offered)?;
    blocking(library, move |library| library.vouched(session, reported)).await?;
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
        let Proposal { readers, classes } = serde_json::from_slice(&proposal).map_err(|error| error.to_string())?;
        let Reply::Described { descriptor } = link.assets(Request::Describe { scope: scope.clone(), readers, classes }).await? else {
            return Err("the hub did not describe the scope".into());
        };
        if !assets.heard(hub, key, &descriptor.fact.0, &descriptor.word.0) {
            return Err(format!("the descriptor of {scope} is not the hub's"));
        }
        let read: wire::Descriptor = serde_json::from_slice(&descriptor.fact.0).map_err(|error| error.to_string())?;
        blocking(library, move |library| library.install(&scope, descriptor.fact.0, read.seq as i64)).await?;
    }
    Ok(())
}

/// The frames of this machine's own streams of `scope` past what the hub
/// has.
async fn upload(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, session: u64, scope: &str) -> Result<(), String> {
    let library = &assets.library;
    let device = assets.device();
    let key_text = scope.to_string();
    for (stream, from) in blocking(library, move |library| library.uploads(&key_text, &device)).await? {
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
                    let Reply::Received { received } = link.assets(request).await? else {
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
    }
    Ok(())
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
/// the refusal took out is committed again in it.
async fn refused(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, stream: &str, cut: u64) -> Result<(), String> {
    let library = &assets.library;
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    blocking(library, move |library| library.rejected(&scope_text, &stream_text, cut)).await?;
    // The chain value the new epoch follows: where the stream stands now.
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    let standing = blocking(library, move |library| library.standing(&scope_text)).await?;
    let head = standing
        .into_iter()
        .find(|(name, sequence, _)| name == &stream_text && sequence + 1 == cut)
        .map_or([0u8; 32], |(_, _, head)| head);
    let request = Request::Epoch { scope: scope.to_string(), stream: stream.to_string(), next: cut, head: head.into() };
    let Reply::Epoch { descriptor } = link.assets(request).await? else {
        return Err("the hub did not open a new epoch".into());
    };
    if !assets.heard(hub, key, &descriptor.fact.0, &descriptor.word.0) {
        return Err("the new epoch's descriptor is not the hub's".into());
    }
    let fact = EpochFact::decode(&descriptor.fact.0)?;
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    blocking(library, move |library| library.new_epoch(&scope_text, &stream_text, fact.epoch, descriptor.fact.0)).await?;
    loop {
        let (scope_text, stream_text, signer) = (scope.to_string(), stream.to_string(), assets.signer());
        let committed = blocking(library, move |library| library.resubmit(&scope_text, &stream_text, 64, signer)).await?;
        if committed == 0 {
            return Ok(());
        }
    }
}

/// The frames of the streams of `scope` this machine holds, past what it
/// has, under the hub's signed epochs and gap facts.
async fn pull(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, holding: i64) -> Result<(), String> {
    let library = &assets.library;
    let Reply::Streams { streams } = link.assets(Request::Streams { scope: scope.to_string() }).await? else {
        return Err("the hub did not list the scope's streams".into());
    };
    let scope_text = scope.to_string();
    let local: std::collections::BTreeMap<String, u64> =
        blocking(library, move |library| library.streams(&scope_text)).await?.into_iter().collect();
    for wire::StreamAt { stream, next } in streams {
        let mut at = local.get(&stream).copied().unwrap_or(0);
        if at >= next {
            continue;
        }
        let Reply::Succession { epochs, gaps } =
            link.assets(Request::Succession { scope: scope.to_string(), stream: stream.clone() }).await?
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
            let Reply::Frames { frames } = link.assets(request).await? else {
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
    }
    Ok(())
}

/// Where each stream of `scope` stands, checked with the hub; one that
/// differs is cut back to the last place they share (design §7.5).
async fn cursors(library: &Library, link: &Arc<Link>, scope: &str, holding: i64) -> Result<(), String> {
    let scope_text = scope.to_string();
    let standing = blocking(library, move |library| library.standing(&scope_text)).await?;
    if standing.is_empty() {
        return Ok(());
    }
    let streams = standing
        .into_iter()
        .map(|(stream, sequence, head)| wire::Cursor { stream, sequence, head: head.into() })
        .collect();
    let Reply::Standing { streams } = link.assets(Request::Cursors { scope: scope.to_string(), holding, streams }).await? else {
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
