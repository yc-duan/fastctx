//! Serialize history changes while standalone ledgers are handed to their hub.

pub(crate) mod archive;
pub(crate) mod limits;
use crate::Node;
use fastctx_types::history::Scope;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};

#[derive(Default)]
pub(crate) struct Imports {
    gates: Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
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
