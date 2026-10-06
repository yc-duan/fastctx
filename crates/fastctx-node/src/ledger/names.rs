//! Projects by the ids the wire knows them by (`fastctx_net::ledger::TownId`).
//!
//! A project's id is worked out from its name with the World's naming key,
//! which every member receives sealed in the World's own ledger: so the id
//! is the same on every machine and the same name always gives the same id,
//! while the hub, which never has the key, learns nothing from it. The key
//! is kept only in memory and opened again from that ledger's copy here
//! each time the node starts.

use std::sync::Arc;

use fastctx_net::ledger::{self, Item, NamingKey, TownId, WorldEntry};
use fastctx_net::link::Link;
use fastctx_net::sign::Domain;
use fastctx_types::TownName;
use jiff::Timestamp;

use super::items::Items;
use crate::Node;

/// The id of the project `town`, when this machine has the naming key.
pub fn town_id(node: &Node, town: &TownName) -> Option<TownId> {
    let key = node.naming.read().expect("never poisoned").clone()?;
    Some(ledger::town_id(&key, town))
}

/// The id of the project `town`, or why there is none yet.
pub fn require(node: &Node, town: &TownName) -> Result<TownId, String> {
    town_id(node, town).ok_or_else(|| {
        format!(
            "{} has not received the World's naming key yet; it comes sealed for it from a machine with management rights",
            node.machine()
        )
    })
}

/// The projects this machine keeps, by their ids.
pub fn known(node: &Node) -> std::collections::BTreeMap<TownId, TownName> {
    let Some(key) = node.naming.read().expect("never poisoned").clone() else { return Default::default() };
    node.world.towns().unwrap_or_default().into_iter().map(|town| (ledger::town_id(&key, &town), town)).collect()
}

/// A scope with its projects named by their ids, from one naming them by
/// their short names.
pub fn by_id(node: &Node, scope: fastctx_types::scope::NamedScope) -> Result<fastctx_types::scope::Scope, String> {
    let key = node.naming.read().expect("never poisoned").clone();
    scope.named(|town| key.as_ref().map(|key| ledger::town_id(key, town))).ok_or_else(|| {
        format!("{} has not received the World's naming key yet, so it cannot name projects in a scope", node.machine())
    })
}

/// A scope as a person reads it: projects by their short names where this
/// machine knows them, by their ids where it does not.
pub fn by_name(node: &Node, scope: &fastctx_types::scope::Scope) -> fastctx_types::scope::Scope<String> {
    let known = known(node);
    scope
        .named(|id| Some(known.get(id).map(|town| town.to_string()).unwrap_or_else(|| id.to_string())))
        .expect("every project has a name to show")
}

pub(super) fn set(node: &Node, key: NamingKey) {
    *node.naming.write().expect("never poisoned") = Some(key);
}

/// Opens the naming key from the copy of the World's own ledger kept here.
pub fn load(node: &Arc<Node>, link: &Link) {
    if node.naming.read().expect("never poisoned").is_some() {
        return;
    }
    let Ok(items) = Items::new(&node.world).all(&TownId::world()) else { return };
    for (_, item, _) in &items {
        let Item::Entry(entry) = item else { continue };
        let shares = items.iter().filter_map(|(_, item, _)| match item {
            Item::Share(share) => Some(share),
            Item::Entry(_) => None,
        });
        let Ok(Some(key)) = link.entry_key(entry, shares) else { continue };
        let Ok(inner) = ledger::sealed::open_entry(entry, &key) else { continue };
        if let Ok(world) = inner.claim::<WorldEntry>() {
            set(node, world.naming);
            return;
        }
    }
}

/// Makes the World's naming key and writes it into the World's own ledger,
/// when this machine holds management rights and the World has none yet.
/// Two machines doing so at once write one key: the hub takes the first.
pub async fn ensure(node: &Arc<Node>, link: &Link) -> Result<(), String> {
    if node.naming.read().expect("never poisoned").is_some() || !link.me().is_some_and(|me| me.manage) {
        return Ok(());
    }
    let at_hub = link.ledger_towns().await?;
    if at_hub.iter().any(|(town, _)| *town == TownId::world()) {
        return Ok(());
    }
    let membership = link.membership();
    let naming = NamingKey::generate().map_err(|error| error.to_string())?;
    let entry =
        WorldEntry { world: membership.world.clone(), author: membership.name.clone(), at: Timestamp::now(), naming };
    let inner = link.sign(Domain::Ledger, &entry);
    let item = link.seal_entry(&TownId::world(), None, &[], None, &inner)?;
    match link.ledger_write(item).await {
        Ok(_) => Ok(()),
        // Another machine wrote it first; it is taken with the World's ledger.
        Err(error) if error.contains("nothing in a ledger is written twice") => Ok(()),
        Err(error) => Err(error),
    }
}
