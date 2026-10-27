//! Conversation deletion records its sentinel cleanup before forgetting identity.
use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, HubAccount},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};
#[derive(Serialize, Deserialize)]
struct Removal {
    owners: Vec<Address>,
    pending: BTreeSet<HubAccount>,
}

pub(crate) fn note_deleted(node: &Node, id: &ResidentId) -> Result<(), String> {
    let acting = crate::here::acting(node, id).ok_or("the deleted conversation's account identity is unavailable")?;
    let mut pending: BTreeSet<_> = match &acting {
        Acting::Accounts { accounts } => accounts.iter().cloned().collect(),
        Acting::Here => BTreeSet::new(),
    };
    for link in node.links() {
        for account in node.accounts_on(&link, &acting) {
            pending.insert(HubAccount { hub: link.membership().id.clone(), account });
        }
    }
    let names = node.machine_names();
    let owners = std::iter::once(names.name)
        .chain(names.was)
        .map(|machine| Address { machine, resident: id.clone() })
        .collect::<Vec<_>>();
    fastctx_agent::state::save(&node.world.path().join("sentinel-deleted-owners").join(format!("{id}.json")), &owners)?;
    fastctx_agent::state::save(
        &node.world.path().join("sentinel-owner-removals").join(format!("{id}.json")),
        &Removal { owners, pending },
    )
}

pub(crate) async fn recover(node: &Arc<Node>) -> Vec<String> {
    let mut errors = Vec::new();
    let directory = node.world.path().join("sentinel-owner-removals");
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return errors,
        Err(e) => {
            return vec![format!("reading sentinel owner removals: {e}")];
        }
    };
    for entry in entries {
        let result = async {
            let path = entry.map_err(|e| e.to_string())?.path();
            let mut removal: Removal =
                fastctx_agent::state::load(&path)?.ok_or("sentinel owner removal disappeared")?;
            for scope in removal.pending.clone() {
                let records = node.sentinels.records().map_err(|e| e.message)?;
                for owner in &removal.owners {
                    let unconfirmed = fastctx_sentinel::lifecycle::delete_owner(
                        records,
                        &super::transport::Transport(node.clone()),
                        &scope,
                        owner,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    for reason in unconfirmed {
                        errors.push(format!("sentinel definition withdrawn; home has not confirmed: {reason}"));
                    }
                }
                removal.pending.remove(&scope);
                fastctx_agent::state::save(&path, &removal)?;
            }
            std::fs::remove_file(path).map_err(|e| e.to_string())
        }
        .await;
        if let Err(error) = result {
            errors.push(format!("sentinel owner removal remains pending: {error}"));
        }
    }
    errors
}
