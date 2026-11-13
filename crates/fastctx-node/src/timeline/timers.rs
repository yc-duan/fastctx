//! A conversation's timers in the asset library (design §28.22, plan 3-4
//! group 9): each timer it or its goal set, as the machine that fires it
//! keeps it, written where it changed; one done or cancelled let go. Looked
//! at by the timeline's job, which a timer set, fired or cancelled asks for.
//! A sentinel's timers go with the sentinel, not here.

use super::beside::{Beside, writable};
use crate::Node;
use fastctx_content::v2::{ConversationTimer, Kind as _, Write, Writer};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::timers::Owner;
use std::collections::BTreeMap;
use std::sync::Arc;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// A timer of `owner` changed: its conversation's timers are looked at again.
pub(crate) fn changed(node: &Node, owner: &Owner) {
    if let Some(resident) = owner.conversation() {
        super::tail::wanted(node, resident);
    }
}

/// Brings `resident`'s timers in the asset library to the timer table here;
/// how many were written.
pub(super) async fn produce(
    node: &Arc<Node>,
    resident: &ResidentId,
    producing: &super::serial::Serial,
) -> Result<usize, Failure> {
    let held = producing.hold(resident).await;
    let Ok(store) = crate::timers::table(node) else { return Ok(0) };
    let Some(Beside { library, scope, holding }) = writable(node, resident, ConversationTimer::STREAM, &held).await?
    else {
        return Ok(0);
    };
    let mut wanted = BTreeMap::new();
    for owner in [Owner::Conversation { resident: resident.clone() }, Owner::Goal { resident: resident.clone() }] {
        for timer in store.list(&owner).await.map_err(failure)? {
            wanted.insert(timer.id.clone(), serde_json::to_vec(&timer).map_err(failure)?);
        }
    }
    let signer = node.assets.as_ref().map(|assets| assets.signer()).ok_or_else(|| failure("no asset library"))?;
    fastctx_platform::task::blocking(move || {
        let _held = held;
        let mut writes = Vec::new();
        let mut after = None;
        loop {
            let page = library.page(&scope, ConversationTimer::STREAM, after.clone(), 256).map_err(failure)?;
            let Some(last) = page.last() else { break };
            after = Some((last.order.clone(), last.item.clone()));
            for entry in page {
                let id = String::from_utf8_lossy(&entry.item).into_owned();
                match wanted.remove(&id) {
                    Some(body) if body == entry.body => {}
                    Some(body) => writes.push(Write::<ConversationTimer> {
                        order: entry.item.clone(),
                        item: entry.item,
                        value: Some(body),
                        objects: Vec::new(),
                    }),
                    None => writes.push(Write::<ConversationTimer> {
                        order: entry.item.clone(),
                        item: entry.item,
                        value: None,
                        objects: Vec::new(),
                    }),
                }
            }
        }
        for (id, body) in wanted {
            let item = id.into_bytes();
            writes.push(Write::<ConversationTimer> { order: item.clone(), item, value: Some(body), objects: Vec::new() });
        }
        if writes.is_empty() {
            return Ok(0);
        }
        let count = writes.len();
        Writer::<ConversationTimer>::new(&library, &scope, ConversationTimer::STREAM)
            .map_err(failure)?
            .in_holding(holding)
            .write_all(writes, Vec::new(), false, signer)
            .map_err(failure)?;
        Ok::<_, Failure>(count)
    })
    .await
}
