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
use fastctx_content::v2::{ConversationWords, Delivery as Way, Entry, Kind as _, Op, Write, Writer};
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

/// The most words on their way followed at once.
const OPEN_AT_ONCE: usize = 512;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// An item of words as its record holds it.
#[derive(Serialize, Deserialize)]
struct Said {
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
    let digest = Sha256::digest(serde_json::to_vec(&delivery.notice).map_err(failure)?).into();
    Ok(Op { id: delivery.id.to_string(), digest, kind: "words".into() })
}

fn body(state: &str, delivery: &Delivery, why: Option<String>) -> Result<Vec<u8>, Failure> {
    serde_json::to_vec(&Said { state: state.into(), delivery: delivery.clone(), why }).map_err(failure)
}

/// What became of writing words.
enum Wrote {
    Written,
    /// Not here yet, or ordered by another device: the inbox keeps them, and
    /// the job takes them up later.
    Skipped,
    Wanted,
}

/// Keeps the user's words `delivery` for `to` in the asset library, with
/// their operation, before the inbox keeps them. Said again under the same
/// operation, nothing is written again. What goes wrong is told and leaves
/// the inbox to keep them alone, as before (§28.14).
pub(crate) async fn said(node: &Arc<Node>, to: &ResidentId, delivery: &Delivery) {
    if !is_words(delivery) {
        return;
    }
    if let Err(error) = keep(node, to, delivery).await {
        node.log.line(format_args!("{to}'s words {} in the asset library: {}", delivery.id, error.message));
    }
}

async fn keep(node: &Arc<Node>, to: &ResidentId, delivery: &Delivery) -> Result<(), Failure> {
    let Some(kept) = node.assets.as_ref() else { return Ok(()) };
    let Some((about, _)) = node.residents.about(to) else { return Ok(()) };
    let Some(placed) = placed(node, &about)? else { return Ok(()) };
    // Let go to another machine with its export: the inbox keeps them
    // alone (J08).
    if crate::assets::release::released_away(node.world.conversation(to).path(), &placed.scope, &kept.device()) {
        return Ok(());
    }
    // Come from another machine, its scope is begun here by the hub's
    // description alone (AG10).
    let born_elsewhere = node.residents.birth_machine(to).is_some_and(|born| !node.is_me(&born));
    let (library, signer, device) = (kept.library.clone(), kept.signer(), kept.device());
    let (scope, network) = (placed.scope.clone(), placed.network.clone());
    let proposal = serde_json::to_vec(&placed.proposal).map_err(failure)?;
    let (op, value) = (op_of(delivery)?, body("kept", delivery, None)?);
    let (item, order) = (delivery.id.as_str().as_bytes().to_vec(), order(false, delivery.at));
    let wrote = fastctx_platform::task::blocking(move || {
        let holding = match library.holding(&scope)? {
            Some(holding) => holding,
            None if born_elsewhere => return Ok(Wrote::Skipped),
            None => {
                library.propose_on(&network, &scope, proposal)?;
                let Some(holding) = library.holding(&scope)? else { return Ok(Wrote::Skipped) };
                holding
            }
        };
        if born_elsewhere && !library.is_live(&scope)? {
            return Ok(Wrote::Skipped);
        }
        if library.orderer(&scope, ConversationWords::STREAM)?.is_some_and(|orderer| orderer != device) {
            return Ok(Wrote::Wanted);
        }
        let writer = Writer::<ConversationWords>::new(&library, &scope, ConversationWords::STREAM)?.in_holding(holding);
        writer.write(item, order, Some(&value), Vec::new(), Vec::new(), Some((op, true)), signer)?;
        Ok::<_, fastctx_content::Error>(Wrote::Written)
    })
    .await
    .map_err(failure)?;
    if matches!(wrote, Wrote::Wanted) {
        crate::assets::takeover::want(node, &placed.hub, &placed.scope, ConversationWords::STREAM);
    }
    Ok(())
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
        let page = asking.page(&looked, ConversationWords::STREAM, None, OPEN_AT_ONCE)?;
        let open: Vec<Entry> = page.into_iter().take_while(|entry| entry.order.first() == Some(&OPEN)).collect();
        Ok::<_, fastctx_content::Error>((
            asking.holding(&looked)?,
            asking.is_live(&looked)?,
            asking.orderer(&looked, ConversationWords::STREAM)?,
            open,
        ))
    })
    .await
    .map_err(failure)?;
    // Not before the scope is held here; for one from elsewhere, not before
    // the hub described it (AG10).
    let Some(holding) = holding else { return Ok(0) };
    let born_elsewhere = node.residents.birth_machine(resident).is_some_and(|born| !node.is_me(&born));
    if born_elsewhere && !live {
        return Ok(0);
    }
    if orderer.is_some_and(|orderer| orderer != kept.device()) {
        crate::assets::takeover::want(node, &placed.hub, &scope, ConversationWords::STREAM);
        return Ok(0);
    }
    let ids: Vec<String> = open.iter().map(|entry| String::from_utf8_lossy(&entry.item).into_owned()).collect();
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
    for (entry, stood) in open.iter().zip(stood) {
        let said: Said = serde_json::from_slice(&entry.body).map_err(failure)?;
        let (state, settled) = state_of(stood);
        if state == said.state {
            continue;
        }
        let value = (stood != Stood::Gone).then(|| body(state, &said.delivery, None)).transpose()?;
        let order = order(settled, said.delivery.at);
        writes.push(Write::<ConversationWords> { item: entry.item.clone(), order, value, objects: Vec::new() });
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
            let (Ok(op), Ok(value)) = (op_of(&pending.delivery), body(state, &pending.delivery, None)) else {
                continue;
            };
            let order = order(settled, pending.delivery.at);
            // Let go and kept again under the same operation, the words are
            // written again without it: it landed once already.
            let landed = library.durability(vec![op.id.clone()])?.first().is_some_and(|(_, durable)| *durable);
            let op = (!landed).then_some((op, true));
            writer.write(item, order, Some(&value), Vec::new(), Vec::new(), op, signer.clone())?;
        }
        if !writes.is_empty() {
            writer.write_all(writes, Vec::new(), false, signer)?;
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
