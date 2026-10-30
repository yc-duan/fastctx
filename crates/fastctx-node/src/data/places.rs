//! One gate for each conversation's place on this machine, its home or a
//! guest room, by the name of its directory (`specs/world-concurrency.md`,
//! 一段对话的一生). Deleting the place holds its gate alone; starting a
//! command for it, reading a command's output there, receiving a file for it
//! and a Claude session claiming its directory share it. Deleting one
//! conversation holds back nothing of another.

use std::path::Path;

use fastctx_platform::doing;
use fastctx_types::Address;

use crate::Node;
use crate::leases::{Lease, Registry};

#[derive(Default)]
pub(crate) struct Places {
    gates: Registry<String, doing::RwLock<()>>,
}

/// A place's gate shared: what uses the place goes on, and no deletion of it
/// begins meanwhile.
pub(crate) struct Using {
    _guard: doing::OwnedRwLockReadGuard<()>,
    _gate: Lease<String, doing::RwLock<()>>,
}

/// A place's gate held alone, to delete it.
pub(crate) struct Deleting {
    _guard: doing::OwnedRwLockWriteGuard<()>,
    _gate: Lease<String, doing::RwLock<()>>,
}

impl Places {
    fn gate(&self, name: &str) -> Lease<String, doing::RwLock<()>> {
        self.gates.get_or_insert_with(&name.to_string(), || doing::RwLock::new(doing::Rank::PLACE, "place", name, ()))
    }

    /// Shares the gate of the place named `name`.
    pub(crate) async fn using(&self, name: &str) -> Using {
        let gate = self.gate(name);
        let guard = gate.read_owned().await;
        Using { _guard: guard, _gate: gate }
    }

    /// Holds the gate of the place named `name` alone, once what uses it
    /// lets go; `None` when that takes longer than a deletion waits.
    pub(crate) async fn deleting(&self, name: &str) -> Option<Deleting> {
        let gate = self.gate(name);
        let guard = match crate::gate_waits::remaining() {
            Some(within) => tokio::time::timeout(within, gate.write_owned()).await.ok()?,
            None => gate.write_owned().await,
        };
        Some(Deleting { _guard: guard, _gate: gate })
    }
}

/// The place of `owner` here: its home, or its guest room.
pub(crate) fn of_owner(node: &Node, owner: &Address) -> Option<String> {
    name(node.world.owner(&node.names(), owner).path())
}

/// The place a path here lies in: the conversation directory it is under.
pub(crate) fn of_path(node: &Node, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(node.world.conversations()).ok()?;
    rest.components().next()?.as_os_str().to_str().map(str::to_string)
}

fn name(directory: &Path) -> Option<String> {
    directory.file_name()?.to_str().map(str::to_string)
}
