//! One observed plan cursor per conversation, at most 4 KiB. Replacing the
//! cursor retains no history; it lives until the conversation is deleted.
use crate::{Node, records::metadata};
use fastctx_types::{ResidentId, access::HubAccount, ipc::Failure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Seen {
    pub account: HubAccount,
    pub owner: ResidentId,
    pub version: Option<u64>,
}

pub(super) fn read(node: &Node, id: &ResidentId) -> Result<Option<Seen>, Failure> {
    metadata::get(&node.world.conversation(id).path().join("plan-seen.json"))
        .map_err(|error| Failure::new(error.to_string()))
}

pub(super) async fn load(node: &Node, id: &ResidentId) -> Result<Option<Seen>, Failure> {
    let path = node.world.conversation(id).path().join("plan-seen.json");
    fastctx_platform::task::spawn_blocking(move || metadata::get(&path))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(error.to_string()))
}

pub(super) fn keep(node: &Node, id: &ResidentId, observed: &Seen) -> Result<(), Failure> {
    keep_at(&node.world.conversation(id).path().join("plan-seen.json"), observed)
}

fn keep_at(path: &std::path::Path, observed: &Seen) -> Result<(), Failure> {
    let body = serde_json::to_vec(observed).map_err(|error| Failure::new(error.to_string()))?;
    if body.len() > 4096 {
        return Err(Failure::new("the observed plan identity exceeds its 4 KiB budget"));
    }
    let observed = observed.clone();
    metadata::change(path, body.len(), move |rows, family| {
        if rows.get::<Seen>(family, "value")?.is_some_and(|before| {
            before.account == observed.account && before.owner == observed.owner && before.version >= observed.version
        }) {
            return Ok(());
        }
        rows.put_raw(family, "value", &body)
    })
    .map_err(|error| Failure::new(error.to_string()))
}
