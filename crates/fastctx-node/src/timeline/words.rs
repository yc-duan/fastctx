//! The user's words to a conversation in the asset library (design §3,
//! §28.14): written as they arrive, before the inbox keeps them, with their
//! operation in the same transaction that waits for the disk; then where
//! their delivery stands, followed from the inbox each time the
//! conversation's timeline job runs (`produce`), written only where it
//! changed. The inbox still delivers them; the asset library is where they
//! are first kept and how other devices see them.
//!
//! An item's order begins with whether its words are settled, then when they
//! were said: what is still on its way comes first, so a page from the start
//! up to the first settled item is all of it.

use super::assets::placed;
use crate::Node;
use crate::inbox::standing::Stood;
use fastctx_content::v2::{ConversationWords, Delivery as Way, Entry, Kind as _, Library, Op, Write, Writer};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Delivery, Notice, Pending};
use fastctx_types::session::Speaker;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// What leads the order of words still on their way, and of settled ones.
const OPEN: u8 = 0;
const SETTLED: u8 = 1;

/// The most words on their way read in one page.
const OPEN_AT_ONCE: usize = 500;

/// How long words are seen nowhere in the inbox, by this node as it runs,
/// before they are taken as let go: not from when they were said, which an
/// old notice may be long past (NN03).
const GONE_AFTER: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// When this node first saw each of the words on their way nowhere in the
/// inbox.
static MISSING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> =
    std::sync::LazyLock::new(Default::default);

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// An item of words as its record holds it.
#[derive(Serialize, Deserialize)]
struct Body {
    state: String,
    delivery: Delivery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    why: Option<String>,
}

/// Whether `delivery` is the user's words: what the asset library keeps.
pub(crate) fn is_words(delivery: &Delivery) -> bool {
    matches!(
        &delivery.notice,
        Notice::Words { from: Speaker::User { .. }, goal_report: None, goal_question: None, told: None, .. }
            | Notice::Attached { .. }
    )
}

/// The name of where words stand, and whether they are settled there.
fn state_of(stood: Stood) -> (&'static str, bool) {
    match stood {
        Stood::Kept => ("kept", false),
        Stood::Delivering => ("delivering", false),
        Stood::Uncertain => ("uncertain", false),
        Stood::Delivered => ("delivered", true),
        Stood::Refused => ("refused", true),
        Stood::Gone => ("gone", true),
    }
}

/// An item's order: settled or not, then when the words were said.
fn order(settled: bool, at: jiff::Timestamp) -> Vec<u8> {
    let mut order = vec![if settled { SETTLED } else { OPEN }];
    order.extend_from_slice(&((at.as_nanosecond() as u128) ^ (1 << 127)).to_be_bytes());
    order
}

/// The operation words arrive under: their notice id, and the digest of
/// what they say.
fn op_of(delivery: &Delivery) -> Result<Op, Failure> {
    Ok(Op { id: delivery.id.to_string(), digest: digest_of(&delivery.notice)?, kind: "words".into() })
}

/// What words say, as their operation's digest: the machines they name
/// left out — a machine's name changes, the words do not — so the same words
/// sent again after a rename are the same request (OO05).
fn digest_of(notice: &Notice) -> Result<[u8; 32], Failure> {
    fn unnamed(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.remove("machine");
                // The machine words with files attached came from (PP04).
                if map.contains_key("delivery") && map.contains_key("attachments") {
                    map.remove("from");
                }
                map.values_mut().for_each(unnamed);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(unnamed),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(notice).map_err(failure)?;
    unnamed(&mut value);
    Ok(Sha256::digest(serde_json::to_vec(&value).map_err(failure)?).into())
}

/// The most a body holds as it is: words larger keep their delivery as the
/// one object, their body saying where they stand and that it is so.
const BODY_MOST: usize = fastctx_content::RECORD_BYTES - 4096;

/// A record's body of words standing at `state`, with the objects it refers
/// to and the leases that keep a new one until it is committed.
type Stored = (Vec<u8>, Vec<fastctx_content::ObjectRef>, Vec<fastctx_content::v2::Lease>);

fn stored(library: &Library, state: &str, delivery: &Delivery, why: Option<String>) -> fastctx_content::Result<Stored> {
    let invalid = |error: serde_json::Error| fastctx_content::Error::Invalid(error.to_string());
    let whole = serde_json::to_vec(&Body { state: state.into(), delivery: delivery.clone(), why: why.clone() }).map_err(invalid)?;
    if whole.len() <= BODY_MOST {
        return Ok((whole, Vec::new(), Vec::new()));
    }
    let delivered = serde_json::to_vec(delivery).map_err(invalid)?;
    let lease = library.put(&mut delivered.as_slice(), "words", fastctx_content::v2::LEASE)?;
    let said = serde_json::json!({ "state": state, "why": why, "in_object": true, "id": delivery.id, "at": delivery.at });
    Ok((serde_json::to_vec(&said).map_err(invalid)?, vec![lease.object], vec![lease]))
}

/// The words `entry` holds: its body, or, kept as an object, its delivery
/// read from it.
fn held_body(library: &Library, entry: &Entry) -> Option<Body> {
    if let Ok(body) = serde_json::from_slice::<Body>(&entry.body) {
        return Some(body);
    }
    let value: serde_json::Value = serde_json::from_slice(&entry.body).ok()?;
    if value.get("in_object").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    let object = entry.objects.first()?;
    let mut bytes = Vec::with_capacity(object.size as usize);
    while (bytes.len() as u64) < object.size {
        let piece = library.read(&object.hash, bytes.len() as u64, 1 << 20).ok()?;
        if piece.is_empty() {
            return None;
        }
        bytes.extend_from_slice(&piece);
    }
    let delivery: Delivery = serde_json::from_slice(&bytes).ok()?;
    Some(Body {
        state: value.get("state")?.as_str()?.to_string(),
        delivery,
        why: value.get("why").and_then(serde_json::Value::as_str).map(str::to_string),
    })
}

/// Where words kept as an object stand, as their record says, though the
/// object is not here.
fn kept_state(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(body).ok()?.get("state")?.as_str().map(str::to_string)
}

/// What became of writing words.
enum Wrote {
    Written,
    /// Not here yet, or ordered by another device: the inbox keeps them, and
    /// the job takes them up later.
    Skipped,
    Wanted,
    /// Held here already — taken from the machine they were said to before,
    /// or kept here before — as having crossed into the host (MM01).
    Crossed,
    /// Held here already as not known to have reached the host, or kept by
    /// the machine they were said to before (NN01, NN04).
    Uncertain(String),
}

/// What keeping the user's words in the asset library says of handing them
/// on.
pub(crate) enum Said {
    /// Kept, or kept by the inbox alone: they go on to the host.
    Go,
    /// They crossed into the host already, here or where the conversation
    /// lived before: they are not handed over again.
    Crossed,
    /// Whether they reached the host is not known: they are not handed over
    /// again, and the user says whether to send them anew (NN01, NN04).
    Uncertain(String),
}

/// Keeps the user's words `delivery` for `to` in the asset library, with
/// their operation, before the inbox keeps them (§28.14). Said again under
/// the same operation, nothing is written again; under the same number with
/// other words, they are refused (AQ13, MM04); held as having crossed into
/// the host already, they are not handed over again (MM01). A write that
/// fails refuses them: what is kept is kept here first (K01, MM05).
pub(crate) async fn said(node: &Arc<Node>, to: &ResidentId, delivery: &Delivery) -> Result<Said, Failure> {
    if !is_words(delivery) {
        return Ok(Said::Go);
    }
    match keep(node, to, delivery).await {
        Ok(Wrote::Crossed) => Ok(Said::Crossed),
        Ok(Wrote::Uncertain(why)) => Ok(Said::Uncertain(why)),
        Ok(_) => Ok(Said::Go),
        Err(failure) if failure.message.contains(fastctx_content::v2::OP_REUSED) => {
            Err(Failure::refused(format!("{}: those words were sent before as other words under that id", delivery.id)))
        }
        Err(failure) => {
            node.log.line(format_args!("{to}'s words {} in the asset library: {}", delivery.id, failure.message));
            Err(Failure::new(format!("keeping these words: {}", failure.message)))
        }
    }
}

async fn keep(node: &Arc<Node>, to: &ResidentId, delivery: &Delivery) -> Result<Wrote, Failure> {
    let Some(kept) = node.assets.as_ref() else { return Ok(Wrote::Skipped) };
    // Kept here before under this number as other words: refused, before
    // anything else is looked at, whatever else holds (AR16, NN02, L01, AS05).
    let (checking, op) = (kept.library.clone(), op_of(delivery)?);
    let digest = op.digest;
    let reused = fastctx_platform::task::blocking(move || checking.op_digest(&op.id))
        .await
        .map_err(failure)?
        .is_some_and(|kept| kept != digest);
    if reused {
        return Err(Failure::new(fastctx_content::v2::OP_REUSED));
    }
    let Some((about, _)) = node.residents.about(to) else { return Ok(Wrote::Skipped) };
    // One at a time with the conversation's producers: where the words stand
    // is never written over on what was read before they were said again
    // (OO03); and where they go is chosen then, as a producer that went
    // first left it (PP08).
    let _producing = super::tail::PRODUCING.hold(to).await;
    let Some(placed) = placed(node, &about)? else { return Ok(Wrote::Skipped) };
    // Let go to another machine with its export: the inbox keeps them
    // alone (J08), once what is held here of this number was looked at
    // (PP03).
    let released = crate::assets::release::released_away(node.world.conversation(to).path(), &placed.scope, &kept.device());
    // Come from another machine, its scope is begun here by the hub's
    // description alone (AG10), and nothing is written to it before every
    // stream of it was pulled here once: what orders each is known (MM02).
    let born_elsewhere = node.residents.birth_machine(to).is_some_and(|born| !node.is_me(&born));
    let (library, signer, device) = (kept.library.clone(), kept.signer(), kept.device());
    let (scope, network) = (placed.scope.clone(), placed.network.clone());
    let proposal = serde_json::to_vec(&placed.proposal).map_err(failure)?;
    let (op, said) = (op_of(delivery)?, delivery.clone());
    let (item, order) = (delivery.id.as_str().as_bytes().to_vec(), order(false, delivery.at));
    let refused = || fastctx_content::Error::Invalid(fastctx_content::v2::OP_REUSED.to_string());
    let wrote = fastctx_platform::task::blocking(move || {
        // Kept here before under this number as other words: refused,
        // whatever else holds (AR16, NN02, L01).
        if library.op_digest(&op.id)?.is_some_and(|digest| digest != op.digest) {
            return Err(refused());
        }
        // Held here already: crossed into the host, they go no further;
        // still kept or let go, they go on — let go to another machine or
        // not (PP03).
        let ours = library.op_digest(&op.id)?.is_some();
        if let Some(entry) = library.item(&scope, ConversationWords::STREAM, &item)? {
            let held = held_body(&library, &entry);
            let state = held.as_ref().map(|held| held.state.clone()).or_else(|| kept_state(&entry.body)).unwrap_or_default();
            let why = held.as_ref().and_then(|held| held.why.clone());
            if !ours {
                match &held {
                    // Kept elsewhere under this number as other words:
                    // refused too.
                    Some(held) if digest_of(&held.delivery.notice).ok() != Some(op.digest) => return Err(refused()),
                    Some(_) => {}
                    // What was kept under this number is not all here yet:
                    // whether these are those words is not known, and they
                    // are not taken for new ones (PP01, N05).
                    None => {
                        return Ok(Wrote::Uncertain(
                            "words were kept under this id that are not all here yet: whether these are they is not known"
                                .into(),
                        ));
                    }
                }
            }
            match state.as_str() {
                "delivering" | "delivered" => return Ok(Wrote::Crossed),
                "uncertain" => {
                    return Ok(Wrote::Uncertain(
                        why.unwrap_or_else(|| "whether these words reached the agent is not known".into()),
                    ));
                }
                // Kept by the machine the conversation lived on before,
                // which may have handed them over already: never handed
                // over again on not seeing them there (NN01, §3 N02).
                "kept" if !ours => {
                    return Ok(Wrote::Uncertain(
                        "these words were kept where the conversation lived before, which may have handed them over"
                            .into(),
                    ));
                }
                _ => {}
            }
        }
        if released {
            return Ok(Wrote::Skipped);
        }
        let holding = match library.holding(&scope)? {
            Some(holding) => holding,
            None if born_elsewhere => return Ok(Wrote::Skipped),
            None => {
                library.propose_on(&network, &scope, proposal)?;
                let Some(holding) = library.holding(&scope)? else { return Ok(Wrote::Skipped) };
                holding
            }
        };
        if born_elsewhere && !(library.is_live(&scope)? && library.is_pulled(&scope)?) {
            return Ok(Wrote::Skipped);
        }
        if library.orderer(&scope, ConversationWords::STREAM)?.is_some_and(|orderer| orderer != device) {
            return Ok(Wrote::Wanted);
        }
        let writer = Writer::<ConversationWords>::new(&library, &scope, ConversationWords::STREAM)?.in_holding(holding);
        // Kept here before and let go since, said again as the same words:
        // on their way again, its operation as it was (NN03).
        let operation = if ours { None } else { Some((op, true)) };
        let (value, objects, leases) = stored(&library, "kept", &said, None)?;
        writer.write(item, order, Some(&value), objects, leases, operation, signer)?;
        Ok::<_, fastctx_content::Error>(Wrote::Written)
    })
    .await
    .map_err(failure)?;
    if matches!(wrote, Wrote::Wanted) {
        crate::assets::takeover::want(node, &placed.hub, &placed.scope, ConversationWords::STREAM);
    }
    // Kept again: when they went missing before counts no more (PP05).
    if matches!(wrote, Wrote::Written) {
        MISSING.lock().expect("missing words").remove(delivery.id.as_str());
    }
    Ok(wrote)
}

/// Brings where `resident`'s words stand in the asset library to what its
/// inbox says, and takes up words the inbox keeps that it does not have;
/// how many items were written.
pub(super) async fn produce(
    node: &Arc<Node>,
    resident: &ResidentId,
    producing: &super::serial::Serial,
) -> Result<usize, Failure> {
    // Held by each piece of blocking work too: one its job gave up on ends
    // before the next build reads (LL16).
    let held = producing.hold(resident).await;
    let Some(kept) = node.assets.as_ref() else { return Ok(0) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(0) };
    let Some(placed) = placed(node, &about)? else { return Ok(0) };
    let (library, scope) = (kept.library.clone(), placed.scope.clone());
    // Let go to another machine with its export: not written here (J08).
    if crate::assets::release::released_away(node.world.conversation(resident).path(), &scope, &kept.device()) {
        return Ok(0);
    }
    let (asking, looked, holds) = (library.clone(), scope.clone(), held.clone());
    let (holding, live, orderer, open) = fastctx_platform::task::blocking(move || {
        let _held = holds;
        // Every item still on its way, a page at a time up to the first
        // settled one (MM06).
        let mut open: Vec<Entry> = Vec::new();
        let mut after = None;
        loop {
            let page = asking.page(&looked, ConversationWords::STREAM, after.clone(), OPEN_AT_ONCE)?;
            let full = page.len() == OPEN_AT_ONCE;
            let before = open.len();
            open.extend(page.into_iter().take_while(|entry| entry.order.first() == Some(&OPEN)));
            let Some(last) = open.last() else { break };
            if !full || open.len() - before < OPEN_AT_ONCE {
                break;
            }
            after = Some((last.order.clone(), last.item.clone()));
        }
        // Each with what it says and whether its operation is this
        // machine's: one kept where the conversation lived before is never
        // let go here on not being in this inbox (M05).
        let mut resolved: Vec<(Entry, Option<Body>, bool)> = Vec::with_capacity(open.len());
        for entry in open {
            let body = held_body(&asking, &entry);
            let ours = asking.op_digest(&String::from_utf8_lossy(&entry.item))?.is_some();
            resolved.push((entry, body, ours));
        }
        let open = resolved;
        Ok::<_, fastctx_content::Error>((
            asking.holding(&looked)?,
            asking.is_live(&looked)? && asking.is_pulled(&looked)?,
            asking.orderer(&looked, ConversationWords::STREAM)?,
            open,
        ))
    })
    .await
    .map_err(failure)?;
    // Not before the scope is held here; for one from elsewhere, not before
    // the hub described it and every stream of it was pulled here once
    // (AG10, MM02).
    let Some(holding) = holding else { return Ok(0) };
    let born_elsewhere = node.residents.birth_machine(resident).is_some_and(|born| !node.is_me(&born));
    if born_elsewhere && !live {
        return Ok(0);
    }
    if orderer.is_some_and(|orderer| orderer != kept.device()) {
        crate::assets::takeover::want(node, &placed.hub, &scope, ConversationWords::STREAM);
        return Ok(0);
    }
    let ids: Vec<String> = open.iter().map(|(entry, _, _)| String::from_utf8_lossy(&entry.item).into_owned()).collect();
    let directory = node.world.conversation(resident);
    let holds = held.clone();
    let crate::inbox::standing::Standing { asked: stood, others } = fastctx_platform::task::blocking(move || {
        let _held = holds;
        crate::inbox::standing::words(&directory, &ids, |pending: &Pending| is_words(&pending.delivery))
    })
    .await
    .map_err(failure)?;
    let mut writes = Vec::new();
    let mut ways = Vec::new();
    for ((entry, said, ours), stood) in open.iter().zip(stood) {
        let Some(said) = said else { return Err(failure(format!("{resident}'s words {} cannot be read", String::from_utf8_lossy(&entry.item)))) };
        // Nothing of them in the inbox yet is not their being let go: kept
        // here just now, the inbox keeps them next (MM03, K02). Only long
        // after this node first saw them missing, they are taken as let go;
        // their sender, keeping them until they landed, sends them again
        // meanwhile, and said again they are on their way again (NN03).
        let id = said.delivery.id.to_string();
        if stood == Stood::Gone {
            let first = *MISSING.lock().expect("missing words").entry(id.clone()).or_insert_with(std::time::Instant::now);
            if first.elapsed() < GONE_AFTER {
                continue;
            }
        }
        MISSING.lock().expect("missing words").remove(&id);
        // Not this machine's and nowhere here: whether they went in where
        // they were kept before is not known, never let go (M05, §3 N02).
        let stood = if stood == Stood::Gone && !ours { Stood::Uncertain } else { stood };
        let (state, settled) = state_of(stood);
        if state == said.state {
            continue;
        }
        let order = order(settled, said.delivery.at);
        writes.push((entry.item.clone(), order, state, said.delivery.clone(), entry.revision));
        match stood {
            Stood::Delivering => ways.push((said.delivery.id.to_string(), Way::Delivering)),
            Stood::Delivered => ways.push((said.delivery.id.to_string(), Way::Delivered)),
            _ => {}
        }
    }
    let count = writes.len() + others.len();
    if count == 0 {
        return Ok(0);
    }
    let signer = kept.signer();
    fastctx_platform::task::blocking(move || {
        let _held = held;
        let writer = Writer::<ConversationWords>::new(&library, &scope, ConversationWords::STREAM)?.in_holding(holding);
        // Taken up from the inbox: each with its operation, as if it had
        // just arrived; one written meanwhile is not written again.
        for (pending, stood) in others {
            let (state, settled) = state_of(stood);
            let item = pending.delivery.id.as_str().as_bytes().to_vec();
            if library.item(&scope, ConversationWords::STREAM, &item)?.is_some() {
                continue;
            }
            let Ok(op) = op_of(&pending.delivery) else { continue };
            let (value, objects, leases) = stored(&library, state, &pending.delivery, None)?;
            let order = order(settled, pending.delivery.at);
            // Let go and kept again under the same operation, the words are
            // written again without it: it landed once already.
            let landed = library.durability(vec![op.id.clone()])?.first().is_some_and(|(_, durable)| *durable);
            let op = (!landed).then_some((op, true));
            writer.write(item, order, Some(&value), objects, leases, op, signer.clone())?;
        }
        if !writes.is_empty() {
            let (mut records, mut all_leases) = (Vec::new(), Vec::new());
            for (item, order, state, delivery, revision) in writes {
                // Said again meanwhile, the words moved on since they were
                // read: what was read stands no more (OO03).
                if library.item(&scope, ConversationWords::STREAM, &item)?.is_none_or(|now| now.revision != revision) {
                    continue;
                }
                let (value, objects, leases) = stored(&library, state, &delivery, None)?;
                records.push(Write::<ConversationWords> { item, order, value: Some(value), objects });
                all_leases.extend(leases);
            }
            writer.write_all(records, all_leases, false, signer)?;
        }
        // Where their operations stand follows; one taken up from before
        // the asset library kept words has no operation to move.
        for (op, way) in ways {
            let _ = library.deliver(&op, way);
        }
        Ok::<_, fastctx_content::Error>(())
    })
    .await
    .map_err(failure)?;
    Ok(count)
}
