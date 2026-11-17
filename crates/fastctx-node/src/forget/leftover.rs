//! What a conversation deleted here left in the asset library.
//!
//! A deletion from before hubs deleted a conversation's scopes with it left
//! the hub describing them, and every machine that held them holding them.
//! Each such conversation — known by its tombstone, and by a scope of it
//! this library still holds — is asked of its hub once more: the hub deletes
//! the scopes and tells the machines holding them, this one among them. An
//! asking the hub took is not made again (`obligations`).

use super::{Ask, keep};
use crate::Node;
use fastctx_types::access::HubId;
use fastctx_types::assets::{Descriptor, Readers};
use fastctx_types::{Address, ResidentId};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the library is looked through at most.
const EVERY: Duration = Duration::from_secs(10 * 60);
static LOOKED: Mutex<Option<Instant>> = Mutex::new(None);

/// Keeps a deletion to ask of its hub for each conversation deleted here
/// lately of which the library still holds a scope.
pub(super) async fn look(node: &Arc<Node>) {
    let Some(assets) = node.assets.as_ref() else { return };
    {
        let mut looked = LOOKED.lock().expect("never poisoned");
        if looked.is_some_and(|at| at.elapsed() < EVERY) {
            return;
        }
        *looked = Some(Instant::now());
    }
    let deleted: BTreeSet<ResidentId> = node.residents.tombstones().into_iter().map(|(id, _)| id).collect();
    if deleted.is_empty() {
        return;
    }
    let library = assets.library.clone();
    let held = match fastctx_platform::task::blocking(move || library.held_scopes()).await {
        Ok(held) => held,
        Err(error) => {
            node.log.line(format_args!("looking for what deleted conversations left in the asset library: {error}"));
            return;
        }
    };
    let machine = node.machine();
    let mut asks = Vec::new();
    for scope in held {
        let Some(id) = scope.key.rsplit('/').next().and_then(|id| id.parse::<ResidentId>().ok()) else { continue };
        if !deleted.contains(&id) || node.residents.about(&id).is_some() {
            continue;
        }
        let Some(described) =
            scope.description.as_deref().and_then(|description| serde_json::from_slice::<Descriptor>(description).ok())
        else {
            continue;
        };
        // One that lived here: born here, or opened here for an account or
        // in a body. Another machine's of the same id is not this one.
        let account = match &described.readers {
            Readers::Conversation { openers, born_on } if born_on.as_ref().is_none_or(|born| node.is_me(born)) => {
                openers.first().cloned()
            }
            Readers::Body { opener, .. } => Some(opener.clone()),
            _ => None,
        };
        let (Some(account), Ok(hub)) = (account, HubId::try_from(described.hub.clone())) else { continue };
        asks.push(Ask { hub, account, conversation: Address { machine: machine.clone(), resident: id }, after: None });
    }
    if asks.is_empty() {
        return;
    }
    let found = asks.len();
    match keep(node, asks).await {
        Ok(()) => node.log.line(format_args!(
            "{found} scope(s) of conversations deleted here are still held; their hubs are asked to delete them"
        )),
        Err(error) => node.log.line(format_args!("keeping what to ask the hubs of deleted conversations' scopes: {error}")),
    }
}
