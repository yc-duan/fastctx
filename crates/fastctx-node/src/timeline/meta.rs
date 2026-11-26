//! A conversation's metadata in the asset library (plan 3-4 group 1): what
//! it is and where it was born, the project it is in, its name, the options
//! it starts again with, and when it was archived — made again from its
//! directory here each time its timeline's job runs, written only where it
//! changed. Made by the machine it lives on, beside its timeline and as the
//! timeline is: not before the hub described a conversation that came from
//! elsewhere, and not while another device orders it — then it is taken
//! over as the timeline is (§28.10).

use super::assets::placed;
use crate::Node;
use fastctx_content::v2::{ConversationMeta, Kind as _, Write, Writer};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use std::collections::HashMap;
use std::sync::Arc;

/// The largest body an item of metadata has: a record holds 64 KiB.
const ITEM_BYTES: usize = 60 * 1024;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Brings `resident`'s metadata in the asset library to what its directory
/// says; how many items were written.
pub(super) async fn produce(
    node: &Arc<Node>,
    resident: &ResidentId,
    producing: &super::serial::Serial,
) -> Result<usize, Failure> {
    // Held by each piece of blocking work too: one its job gave up on ends
    // before the next build reads (LL16).
    let held = producing.hold(resident).await;
    let Some(kept) = node.assets.as_ref() else { return Ok(0) };
    let Some((mut about, town)) = node.residents.about(resident) else { return Ok(0) };
    let Some(placed) = placed(node, &about)? else { return Ok(0) };
    let library = kept.library.clone();
    let scope = placed.scope.clone();
    // Let go to another machine with its export: not written here (J08).
    if crate::assets::release::released_away(node.world.conversation(resident).path(), &scope, &kept.device()) {
        return Ok(0);
    }
    let (asking, looked, holds) = (library.clone(), scope.clone(), held.clone());
    let (holding, live, orderer) = fastctx_platform::task::blocking(move || {
        let _held = holds;
        Ok::<_, fastctx_content::Error>((
            asking.holding(&looked)?,
            asking.is_live(&looked)? && asking.is_pulled(&looked)?,
            asking.orderer(&looked, ConversationMeta::STREAM)?,
        ))
    })
    .await
    .map_err(failure)?;
    // Not before the timeline began the scope here; for one from elsewhere,
    // not before the hub described it (AG10).
    let Some(holding) = holding else { return Ok(0) };
    let born_elsewhere = node.residents.birth_machine(resident).is_some_and(|born| !node.is_me(&born));
    if born_elsewhere && !live {
        return Ok(0);
    }
    if let Some(orderer) = orderer
        && orderer != kept.device()
    {
        crate::assets::takeover::want(node, &placed.hub, &scope, ConversationMeta::STREAM);
        return Ok(0);
    }
    let removed = node.residents.removed(resident);
    let title = node.residents.now(resident).and_then(|now| now.title);
    let (reading, looked, asked, of, holds) =
        (library.clone(), scope.clone(), node.clone(), resident.clone(), held.clone());
    let (options, kept_here) = fastctx_platform::task::blocking(move || {
        let _held = holds;
        let options = crate::session_options::read(&asked, &of)?;
        let here = reading.page(&looked, ConversationMeta::STREAM, None, 16).map_err(failure)?;
        Ok::<_, Failure>((options, here))
    })
    .await?;
    // What it is and where it was born; where it lives now is its place
    // (LL13).
    let lives = about.machine.clone();
    if let Some(born) = node.residents.birth_machine(resident) {
        about.machine = born;
    }
    let wanted: HashMap<&str, Option<Vec<u8>>> = HashMap::from([
        ("about", Some(serde_json::to_vec(&about).map_err(failure)?)),
        (
            "place",
            Some(serde_json::to_vec(&serde_json::json!({ "project": town, "machine": lives })).map_err(failure)?),
        ),
        ("title", title.map(|title| serde_json::to_vec(&serde_json::json!({ "title": title })).unwrap_or_default())),
        (
            "options",
            Some(
                serde_json::to_vec(&serde_json::json!({ "model": options.model, "effort": options.reasoning_effort }))
                    .map_err(failure)?,
            ),
        ),
        ("archived", removed.map(|at| serde_json::to_vec(&serde_json::json!({ "at": at })).unwrap_or_default())),
    ]);
    let mut writes = Vec::new();
    for item in fastctx_content::v2::META_ITEMS {
        let now = kept_here.iter().find(|entry| entry.item == item.as_bytes());
        // One too large for a record — a focus of very many folders — says
        // so, and the others are written all the same (J07).
        let value = wanted.get(item).cloned().flatten().map(|body| {
            if body.len() > ITEM_BYTES {
                serde_json::to_vec(&serde_json::json!({ "omitted": "too large", "bytes": body.len() }))
                    .unwrap_or_default()
            } else {
                body
            }
        });
        let same = match (&value, now) {
            (Some(body), Some(entry)) => entry.body == *body,
            (None, None) => true,
            (None, Some(entry)) => entry.body.is_empty(),
            (Some(_), None) => false,
        };
        if !same {
            writes.push(Write::<ConversationMeta> {
                item: item.as_bytes().to_vec(),
                order: item.as_bytes().to_vec(),
                value,
                objects: Vec::new(),
            });
        }
    }
    if writes.is_empty() {
        return Ok(0);
    }
    let count = writes.len();
    let signer = kept.signer();
    fastctx_platform::task::blocking(move || {
        let _held = held;
        let writer = Writer::<ConversationMeta>::new(&library, &scope, ConversationMeta::STREAM)?.in_holding(holding);
        writer.write_all(writes, Vec::new(), false, signer).map(|_| ())
    })
    .await
    .map_err(failure)?;
    Ok(count)
}
