//! Hand off standalone history without changing its reader before all versions arrive.

mod access;
pub(crate) use access::authorize;
pub(crate) mod archive;
pub(crate) mod copies;
pub(crate) mod limits;
mod replay;
pub(crate) mod state;
mod worker;
use crate::Node;
use fastctx_types::{history::Scope, replicas};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};
pub(crate) use worker::schedule;

#[derive(Default)]
pub(crate) struct Imports {
    gates: Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
    copies: Mutex<BTreeMap<PathBuf, Scope>>,
    work: Mutex<worker::Work>,
    wake: tokio::sync::Notify,
}

pub(crate) async fn lock(node: &Node, scope: &Scope) -> tokio::sync::OwnedMutexGuard<()> {
    let path = super::local_copy::folder(node, scope);
    let gate = {
        let mut gates = node.history_imports.gates.lock().expect("never poisoned");
        gates.retain(|_, gate| gate.strong_count() > 0);
        match gates.get(&path).and_then(Weak::upgrade) {
            Some(gate) => gate,
            None => {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(path, Arc::downgrade(&gate));
                gate
            }
        }
    };
    gate.lock_owned().await
}

pub(crate) fn pending(node: &Node, scope: &Scope) -> bool {
    let pending = !node.links().is_empty()
        && match scope {
            Scope::Conversation { machine, .. } => {
                node.is_me(machine) && state::pending(&super::local::directory(node, scope))
            }
            Scope::Project { project } => crate::ledger::carry::pending(&node.world, project),
        };
    let path = super::local_copy::folder(node, scope);
    let mut copies = node.history_imports.copies.lock().expect("never poisoned");
    if pending {
        copies.insert(path, scope.clone());
    } else {
        copies.remove(&path);
    }
    pending
}

/// A local source must not be overwritten or edited while it is handed to its hub.
pub(crate) fn settling(node: &Node, scope: &replicas::Scope) -> bool {
    let scope = match scope {
        replicas::Scope::Project { name, .. } => Scope::Project { project: name.clone() },
        replicas::Scope::Conversation { conversation } => Scope::Conversation {
            resident: conversation.address.resident.clone(),
            machine: conversation.address.machine.clone(),
        },
    };
    pending(node, &scope)
}

pub(crate) fn writing() -> fastctx_types::ipc::Failure {
    fastctx_types::ipc::Failure::not_run("this history is being imported; try writing again after the import finishes")
}
