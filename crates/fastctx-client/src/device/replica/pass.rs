//! One pass of a device's download with a hub (design §29 3.2): the
//! download half of a node's driver (`fastctx-node` `assets/session.rs`),
//! with none of what a producer does. What a read-only copy needs all the
//! same stays: the session opens with what the device saw of the hub (its
//! pinned service key, the rotations that lead to it, the latest head its
//! word carried), the hub's epochs and gaps go in before frames under them,
//! what the hub lists the device may hold is installed under the hub's word
//! and nothing else, and a scope it may hold no more is let go.

use super::tiers::{self, Hold, RECENT_BEHIND, RECENT_BYTES, Tier};
use super::{REVOKED, Replica, blocking};
use fastctx_content::v2::{
    Backfilled, Header, Holder, Imported, Installed, Network, RawFrame, Signed, Verifier, Window, hub_orders,
};
use fastctx_content::ObjectRef;
use fastctx_net::ids::HubId;
use fastctx_net::link::Link;
use fastctx_net::sign::PublicKey;
use fastctx_types::assets::{self as wire, Bytes, Reply, Request, VERSION};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Frames asked for at once, and records taken of one stream in a turn
/// before the others have theirs.
const FRAMES_AT_ONCE: u32 = 16;
const RECORDS_A_TURN: u64 = 2048;

/// Asks the hub `request`; where it says the device reads the scope no
/// more, its holding here ends (design §4.5, §6.3).
pub(super) async fn ask(replica: &Replica, link: &Arc<Link>, request: Request) -> Result<Reply, String> {
    match link.assets(request).await? {
        Reply::Revoked { scope } => {
            let (key, device) = (scope.clone(), replica.device());
            blocking(&replica.library, move |library| library.revoke_keeping(&key, &device)).await?;
            Err(format!("{REVOKED}{scope}"))
        }
        reply => Ok(reply),
    }
}

/// Opens a session: what the device saw of the hub's history goes first.
/// A device writes nothing, so it reports no receipts.
pub(super) async fn open(replica: &Replica, hub: &HubId, link: &Arc<Link>) -> Result<PublicKey, String> {
    let request = Request::Open {
        version: VERSION,
        seen: replica.pins.seen(hub),
        reported: Vec::new(),
        pinned: replica.pins.pinned(hub),
        lineage: replica.pins.lineage(hub),
    };
    let (offered, rotations, closed) = match link.assets(request).await? {
        Reply::Ready { key, rotations, closed, .. } => (key, rotations, closed),
        Reply::Sealed { why } => return Err(format!("the hub keeps every asset sealed: {why}")),
        _ => return Err("the hub did not open the session".into()),
    };
    let key = replica.pins.pin_following(hub, &offered, &rotations)?;
    if closed {
        // The hub set the history the device saw aside as it recovered:
        // the next pass looks at every scope whole, where each stream is
        // checked against the hub's (codex-058517ed #8).
        replica.pins.forget_seen(hub).map_err(|error| error.to_string())?;
        replica.called.lock().expect("called").entry(hub.clone()).or_default().whole = true;
    }
    Ok(key)
}

/// Installs each scope the hub says the device may hold that it does not
/// hold, or holds under another descriptor, the descriptor the hub's word;
/// all it listed, when one listing under one basis gave them all.
pub(super) async fn discover(
    replica: &Replica,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    network: &Network,
) -> Result<Option<BTreeSet<String>>, String> {
    let library = &replica.library;
    let mut listed = BTreeSet::new();
    let (mut after, mut basis, mut unsettled) = (None, None::<String>, false);
    loop {
        let Reply::Holdings { scopes, next, through, basis: said } = ask(replica, link, Request::Holdings { after: after.clone() }).await?
        else {
            return Err("the hub did not list what this device may hold".into());
        };
        unsettled |= said.is_empty() || basis.as_ref().is_some_and(|basis| *basis != said);
        basis.get_or_insert(said);
        for held in scopes {
            if !replica.pins.heard(hub, key, &held.descriptor.fact.0, &held.descriptor.word.0) {
                return Err(format!("the descriptor of {} is not the hub's", held.scope));
            }
            let read: wire::Descriptor = serde_json::from_slice(&held.descriptor.fact.0).map_err(|error| error.to_string())?;
            if read.scope != held.scope || read.hub != hub.as_str() {
                return Err(format!("the hub listed another scope than {}", held.scope));
            }
            listed.insert(held.scope.clone());
            let (scope, fact) = (held.scope.clone(), held.descriptor.fact.0);
            let here = blocking(library, move |library| library.held_listing(&scope)).await?;
            if here.as_ref().map(|listing| listing.description.as_slice()) != Some(fact.as_slice()) {
                let (network, scope, revision) = (network.clone(), held.scope.clone(), read.seq as i64);
                blocking(library, move |library| library.install_on(&network, &scope, fact, revision)).await?;
            }
            let holds = held.holds.join("\n");
            let readers = serde_json::json!({ "of": held.readers, "frozen": held.frozen }).to_string();
            let kept = here.as_ref().map(|listing| (listing.allowed.as_deref(), listing.readers.as_deref()));
            if kept != Some((Some(holds.as_str()), Some(readers.as_str()))) {
                let scope = held.scope.clone();
                blocking(library, move |library| library.allow(&scope, &holds, &readers)).await?;
            }
        }
        // A device keeps no listing between passes: where one goes on from
        // (`through`) is a node's concern alone.
        let _ = through;
        match next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    Ok((!unsettled).then_some(listed))
}

/// Takes the hub's signed epochs and gap facts of `stream` of `scope`,
/// each checked under the pinned key.
async fn succession(replica: &Replica, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, stream: &str) -> Result<(), String> {
    let library = &replica.library;
    let Reply::Succession { epochs, gaps } =
        ask(replica, link, Request::Succession { scope: scope.to_string(), stream: stream.to_string() }).await?
    else {
        return Err("the hub did not give the stream's succession".into());
    };
    for (fact, is_epoch) in epochs.into_iter().map(|fact| (fact, true)).chain(gaps.into_iter().map(|fact| (fact, false))) {
        if !replica.pins.heard(hub, key, &fact.fact.0, &fact.word.0) {
            return Err("a fact of the stream's succession is not the hub's".into());
        }
        let signed = Signed { fact: fact.fact.0, signature: fact.word.0 };
        let scope_text = scope.to_string();
        if is_epoch {
            blocking(library, move |library| library.accept_epoch(&scope_text, signed, &Trusted)).await?;
        } else {
            blocking(library, move |library| library.accept_gap(&scope_text, signed, &Trusted)).await?;
        }
    }
    Ok(())
}

/// Goes through `scope`, held in `holding`: what the hub withholds goes,
/// what it orders and lags on after a recovery is set aside, and each
/// stream is held as the device's tier holds its kind. Whether a stream
/// has more to take than one turn gave.
pub(super) async fn pull(replica: &Replica, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, holding: i64) -> Result<bool, String> {
    let library = &replica.library;
    let Reply::Streams { streams, withheld } = ask(replica, link, Request::Streams { scope: scope.to_string() }).await? else {
        return Err("the hub did not list the scope's streams".into());
    };
    let scope_text = scope.to_string();
    let mut local: BTreeMap<String, u64> = blocking(library, move |library| library.streams(&scope_text)).await?.into_iter().collect();
    let scope_text = scope.to_string();
    let already = blocking(library, move |library| library.withheld_streams(&scope_text)).await?;
    for stream in withheld.into_iter().filter(|stream| local.get(stream).is_some_and(|at| *at > 0) || !already.contains(stream)) {
        let (scope_text, device) = (scope.to_string(), replica.device());
        blocking(library, move |library| library.withhold(&scope_text, &stream, &device)).await?;
    }
    let theirs: BTreeMap<String, u64> = streams.iter().map(|at| (at.stream.clone(), at.next)).collect();
    for (stream, at) in local.clone() {
        let hub_at = theirs.get(&stream).copied().unwrap_or(0);
        // A stream the hub orders, longer here than the hub has it after it
        // was put back: kept aside, and followed from where the hub is
        // (AS02, AT02). A device gives nothing back of another's.
        if at > hub_at && hub_orders(&stream) {
            let (scope_text, stream_text) = (scope.to_string(), stream.clone());
            let next = blocking(library, move |library| {
                library.set_aside(&scope_text, &stream_text, hub_at, "the hub, put back, holds the stream only before this")
            })
            .await?;
            local.insert(stream, next);
        }
    }
    // Where each stream stands, checked with the hub: one that differs —
    // the hub put back and written on another way, as long as here or
    // longer — is cut back to the last place both hold, or let go of whole
    // where none is, a window from before the hub's history included, and
    // taken again (design §7.5; codex-058517ed #8, #9).
    let (scope_text, device) = (scope.to_string(), replica.device());
    let standing = blocking(library, move |library| library.replica_standing(&scope_text, &device)).await?;
    if !standing.is_empty() {
        let cursors =
            standing.into_iter().map(|(stream, sequence, head)| wire::Cursor { stream, sequence, head: head.into() }).collect();
        let Reply::Standing { streams: checked } =
            ask(replica, link, Request::Cursors { scope: scope.to_string(), holding, streams: cursors }).await?
        else {
            return Err("the hub did not check the cursors".into());
        };
        for standing in checked.into_iter().filter(|standing| !standing.agrees) {
            let ends: Vec<(u64, [u8; 32])> =
                standing.ends.iter().filter_map(|(end, head)| head.array().map(|head| (*end, head))).collect();
            // The hub holds nothing of it: the hub lags, nothing here is cut.
            if ends.is_empty() {
                continue;
            }
            let (scope_text, stream_text) = (scope.to_string(), standing.stream.clone());
            let shared = blocking(library, move |library| library.shared_end(&scope_text, &stream_text, &ends)).await?;
            // What differs is kept aside, whole, before the stream is cut
            // back: it may be the last copy of what the hub signed before it
            // was put back (codex-795fd27c #1); a window's rows go with the cut.
            let (scope_text, stream_text) = (scope.to_string(), standing.stream.clone());
            let from = shared.map_or(0, |through| through + 1);
            blocking(library, move |library| {
                if library.set_aside(&scope_text, &stream_text, from, "differs from the hub's history past where both agree")? > from {
                    library.truncate(&scope_text, &stream_text, shared)?;
                }
                Ok(())
            })
            .await?;
            local.insert(standing.stream, from);
        }
    }
    for at in &streams {
        let (scope_text, stream_text) = (scope.to_string(), at.stream.clone());
        let known = blocking(library, move |library| library.known_last_epoch(&scope_text, &stream_text)).await?;
        // None known here yet, the first epoch's descriptor included: its
        // frames are not taken without it.
        if known.is_none_or(|known| known < at.epoch) {
            succession(replica, hub, link, key, scope, &at.stream).await?;
        }
    }
    // What this device holds as it was read, of an earlier making of the
    // hub's stream, or pages of one that moved on since: pages let go of, to
    // be read anew; a window taken again (codex-058517ed #10, #11).
    let mut anew = BTreeSet::new();
    for at in &streams {
        let (scope_text, stream_text) = (scope.to_string(), at.stream.clone());
        let covered = blocking(library, move |library| library.coverage(&scope_text, &stream_text)).await?;
        if covered.is_empty() {
            continue;
        }
        let earlier = covered.iter().any(|run| run.remade < at.remade);
        match tiers::hold(replica.tier, &at.stream) {
            Hold::Metadata if earlier || covered.iter().any(|run| run.through < at.next) => {
                let (scope_text, stream_text, device) = (scope.to_string(), at.stream.clone(), replica.device());
                blocking(library, move |library| library.drop_cached(&scope_text, &stream_text, &device)).await?;
            }
            Hold::Recent if earlier => {
                anew.insert(at.stream.clone());
            }
            _ => {}
        }
    }
    let mut more = false;
    let mut failed = None;
    for at in &streams {
        let here = local.get(&at.stream).copied().unwrap_or(0);
        match hold(replica, hub, link, key, scope, &at.stream, holding, here, at.next, anew.contains(&at.stream)).await {
            Ok(went_on) => more |= went_on,
            Err(why) if why.starts_with(REVOKED) => return Err(why),
            Err(why) => {
                failed.get_or_insert(format!("{}: {why}", at.stream));
            }
        }
    }
    let scope_text = scope.to_string();
    let now: BTreeMap<String, u64> = blocking(library, move |library| library.streams(&scope_text)).await?.into_iter().collect();
    let caught_up: Vec<String> =
        theirs.iter().filter(|(stream, next)| now.get(*stream).copied().unwrap_or(0) >= **next).map(|(stream, _)| stream.clone()).collect();
    let scope_text = scope.to_string();
    blocking(library, move |library| library.hold_again(&scope_text, holding, caught_up)).await?;
    match failed {
        Some(why) => Err(why),
        None => Ok(more),
    }
}

/// Holds `stream` of `scope` as the device's tier holds its kind: from
/// `here`, where it stands, towards `next`, where the hub said it stands.
/// Whether it has more to take than this turn gave.
#[allow(clippy::too_many_arguments)]
pub(super) async fn hold(
    replica: &Replica,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    scope: &str,
    stream: &str,
    holding: i64,
    mut here: u64,
    next: u64,
    remade: bool,
) -> Result<bool, String> {
    match tiers::hold(replica.tier, stream) {
        // Described only: asked for as it is read.
        Hold::Metadata => return Ok(false),
        // Far behind, or its window of an earlier making: the window, anew.
        Hold::Recent if remade || next.saturating_sub(here) > RECENT_BEHIND => {
            if let Some(head) = window(replica, hub, link, key, scope, stream, holding, remade).await? {
                here = here.max(head);
            }
        }
        _ => {}
    }
    // A recent stream close behind by count, catching up by its frames as
    // far as its bytes allow: past them, its window, then on from there.
    let most = if tiers::hold(replica.tier, stream) == Hold::Recent { RECENT_BYTES } else { u64::MAX };
    let more = match frames(replica, link, scope, stream, holding, here, next, most).await? {
        Some(more) => more,
        None => {
            let at = window(replica, hub, link, key, scope, stream, holding, false).await?;
            let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
            let here = match at {
                Some(head) => head,
                None => blocking(&replica.library, move |library| library.streams(&scope_text))
                    .await?
                    .into_iter()
                    .find(|(name, _)| *name == stream_text)
                    .map_or(here, |(_, next)| next),
            };
            frames(replica, link, scope, stream, holding, here, next, u64::MAX).await?.unwrap_or(true)
        }
    };
    // Holding whole, what is below a window taken before comes too (PH03).
    if replica.tier == Tier::Whole && !more {
        return backfill(replica, link, scope, stream, holding).await;
    }
    Ok(more)
}

/// Installs the hub's window of `stream`, its word checked under the pinned
/// key — `again` over one at its very head, of an earlier making: where its
/// frames go on from, when it was installed.
#[allow(clippy::too_many_arguments)]
async fn window(
    replica: &Replica,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    scope: &str,
    stream: &str,
    holding: i64,
    again: bool,
) -> Result<Option<u64>, String> {
    let Reply::Window { generation, next, head, bytes, remade, low, entries, word } =
        ask(replica, link, Request::Window { scope: scope.to_string(), stream: stream.to_string() }).await?
    else {
        return Err("the hub did not give the stream's window".into());
    };
    let window = Window {
        scope: scope.to_string(),
        stream: stream.to_string(),
        generation,
        next,
        head: head.array().ok_or("a window's head is 32 bytes")?,
        bytes,
        remade,
        low: low.map(|(place, item)| (place.0, item.0)),
        entries: entries.into_iter().map(entry).collect::<Result<_, _>>()?,
    };
    if !replica.pins.heard(hub, key, &window.statement(), &word.0) {
        return Err("the window is not the hub's".into());
    }
    let library = &replica.library;
    match blocking(library, move |library| library.install_window(window, holding, again)).await? {
        Installed::Installed => Ok(Some(next)),
        Installed::Behind => Ok(None),
    }
}

/// An item of a hub's answer as the library takes it.
pub(super) fn entry(entry: wire::Entry) -> Result<fastctx_content::v2::Entry, String> {
    Ok(fastctx_content::v2::Entry {
        item: entry.item.0,
        order: entry.order.0,
        revision: entry.revision,
        kind: entry.kind,
        schema: entry.schema,
        body: entry.body.0,
        objects: entry
            .objects
            .into_iter()
            .map(|(hash, size)| Ok(ObjectRef { hash: hash.array().ok_or("an object's hash is 32 bytes")?, size }))
            .collect::<Result<_, String>>()?,
        local: 0,
    })
}

/// Takes the frames of `stream` of `scope` from `at` towards `next`, at most
/// a turn's records; whether more are left, or none when `most` bytes of
/// frames were taken before it got there.
#[allow(clippy::too_many_arguments)]
async fn frames(
    replica: &Replica,
    link: &Arc<Link>,
    scope: &str,
    stream: &str,
    holding: i64,
    mut at: u64,
    next: u64,
    most: u64,
) -> Result<Option<bool>, String> {
    let library = &replica.library;
    let from = at;
    let mut spent = 0u64;
    while at < next {
        let request = Request::Frames { scope: scope.to_string(), stream: stream.to_string(), from: at, limit: FRAMES_AT_ONCE };
        let Reply::Frames { mut frames, ticket } = ask(replica, link, request).await? else {
            return Err("the hub did not give frames".into());
        };
        bring(link, ticket, &mut frames).await?;
        if frames.is_empty() {
            break;
        }
        let began = at;
        let mut went = false;
        for frame in frames {
            spent += (frame.header.0.len() + frame.signature.0.len() + frame.body.0.len()) as u64;
            let raw = RawFrame { header: frame.header.0, signature: frame.signature.0, body: frame.body.0, objects: Vec::new() };
            let scope_text = scope.to_string();
            match blocking(library, move |library| library.import(&scope_text, raw, &DeviceKeys, Holder::Replica, holding)).await? {
                Imported::Applied { next: reached, .. } => {
                    at = reached;
                    went = true;
                }
                Imported::Already => {}
                Imported::Gap { from, .. } => {
                    went |= from != began;
                    at = from;
                }
                Imported::Conflict => return Err(format!("{stream} of {scope} holds other records at a place the hub gave")),
                Imported::Epoch(epoch) => {
                    return Err(format!("{stream} of {scope} has a frame of epoch {epoch} the hub gave no descriptor of"));
                }
            }
        }
        if !went {
            return Err(format!("{stream} of {scope}: the hub's frames from {began} take it no further"));
        }
        if at < next && spent > most {
            return Ok(None);
        }
        if at < next && at.saturating_sub(from) >= RECORDS_A_TURN {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}

/// Takes what is below a window of `stream` held before, a turn at a time,
/// chained to the window's head (PH03); whether more are left.
async fn backfill(replica: &Replica, link: &Arc<Link>, scope: &str, stream: &str, holding: i64) -> Result<bool, String> {
    let library = &replica.library;
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    let Some(begun) = blocking(library, move |library| library.begin_backfill(&scope_text, &stream_text, holding)).await? else {
        return Ok(false);
    };
    let mut at = begun.at;
    while at < begun.through {
        let request = Request::Frames { scope: scope.to_string(), stream: stream.to_string(), from: at, limit: FRAMES_AT_ONCE };
        let Reply::Frames { mut frames, ticket } = ask(replica, link, request).await? else {
            return Err("the hub did not give frames".into());
        };
        bring(link, ticket, &mut frames).await?;
        if frames.is_empty() {
            return Err(format!("{stream} of {scope}: the hub gave no frames below its window from {at}"));
        }
        for frame in frames {
            let raw = RawFrame { header: frame.header.0, signature: frame.signature.0, body: frame.body.0, objects: Vec::new() };
            let scope_text = scope.to_string();
            match blocking(library, move |library| library.import_backfill(&scope_text, raw, &DeviceKeys, holding)).await? {
                Backfilled::Applied { at: reached } => at = reached,
                Backfilled::Whole => return Ok(false),
                Backfilled::Already => {}
                Backfilled::Gap { from } => at = from,
                Backfilled::Restarted => {
                    return Err(format!("{stream} of {scope}: the frames below its window do not chain to its head; taken again"));
                }
            }
        }
        if at.saturating_sub(begun.at) >= RECORDS_A_TURN {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Brings the bodies of `frames` that travel apart, in order, under
/// `ticket`.
async fn bring(link: &Arc<Link>, ticket: u64, frames: &mut [wire::Frame]) -> Result<(), String> {
    if frames.iter().all(|frame| frame.apart == 0) {
        return Ok(());
    }
    if ticket == 0 {
        return Err("the hub sent frame bodies apart under no ticket".into());
    }
    let mut incoming = link.get_object(ticket).await?;
    let mut expected = 0u64;
    for frame in frames.iter_mut().filter(|frame| frame.apart > 0) {
        if frame.apart > wire::APART_FRAME_BYTES || !frame.body.0.is_empty() {
            return Err("a frame body apart is at most 2 MiB and comes alone".into());
        }
        let mut body = Vec::with_capacity(frame.apart as usize);
        while (body.len() as u64) < frame.apart {
            let (index, bytes) = incoming.chunk().await?;
            if index != expected {
                return Err("frame bodies come in order".into());
            }
            expected += 1;
            body.extend_from_slice(&bytes);
        }
        if body.len() as u64 != frame.apart {
            return Err("a frame body came longer than the hub said".into());
        }
        frame.body = Bytes::from(body);
        frame.apart = 0;
    }
    incoming.done().await
}

/// Takes in a page of `stream` of `scope` below `before`, as it is read
/// (§29 3.4); whether it was taken.
pub(super) async fn older(
    replica: &Replica,
    link: &Arc<Link>,
    scope: &str,
    stream: &str,
    before: Option<(Vec<u8>, Vec<u8>)>,
    limit: u32,
) -> Result<fastctx_content::v2::OlderTaken, String> {
    let library = &replica.library;
    let key = scope.to_string();
    let holding = blocking(library, move |library| library.holding(&key)).await?.ok_or("the scope is not held here")?;
    let asked = blocking(library, |library| library.page_token()).await?;
    let request = Request::Older {
        scope: scope.to_string(),
        stream: stream.to_string(),
        before: before.clone().map(|(place, item)| (Bytes::from(place), Bytes::from(item))),
        limit,
    };
    let Reply::Older { through, entries, more, remade } = ask(replica, link, request).await? else {
        return Err("the hub did not give the page".into());
    };
    let older = fastctx_content::v2::Older {
        scope: scope.to_string(),
        stream: stream.to_string(),
        through,
        before,
        entries: entries.into_iter().map(entry).collect::<Result<_, _>>()?,
        more,
        remade,
    };
    blocking(library, move |library| library.install_older(older, holding, asked)).await
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

impl Verifier for DeviceKeys {
    fn verify(&self, header: &Header, header_bytes: &[u8], signature: &[u8]) -> bool {
        let Ok(signature) = <[u8; 64]>::try_from(signature) else { return false };
        PublicKey::from_bytes(header.key)
            .verify(fastctx_net::sign::Domain::SyncSourceBatch, header_bytes, &fastctx_net::sign::Signature::from_bytes(signature))
            .is_ok()
    }
}
