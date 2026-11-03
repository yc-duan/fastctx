//! Project context dependencies contain only identity and referenced machines.
//! Directory-only changes need no project file reads while those stay authorized.

use super::dependencies::hash;
use crate::Node;
use fastctx_types::{MachineName, TownName, access::Acting, project::Record};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

type Key = (String, TownName);
#[derive(Clone)]
struct Entry {
    place: Option<crate::ledger::names::Place>,
    machines: Vec<MachineName>,
    authority: u64,
    text: u64,
}
#[derive(Default)]
pub(crate) struct Projects(Mutex<BTreeMap<Key, Entry>>);

fn key(acting: &Acting, town: &TownName) -> Key {
    (serde_json::to_string(acting).expect("acting serializes"), town.clone())
}
fn authority(node: &Node, acting: &Acting, town: &TownName, entry: &Entry) -> u64 {
    let permitted =
        crate::town::access::permits_at(node, acting, town, entry.place.as_ref()).map_err(|error| error.message);
    let usable = crate::ledger::query::usable_at(node, acting, entry.place.as_ref().map(|place| &place.hub));
    let names: Vec<_> =
        entry.machines.iter().map(|name| (name, usable.as_ref().is_none_or(|usable| usable(name)))).collect();
    hash(&(permitted, usable.is_some(), names))
}
impl Projects {
    pub(super) fn signature(&self, node: &Node, acting: &Acting, town: &TownName) -> u64 {
        let kept = self.0.lock().expect("never poisoned").get(&key(acting, town)).cloned();
        if let Some(kept) = kept
            && authority(node, acting, town, &kept) == kept.authority
        {
            return kept.text;
        }
        hash(&crate::town::instructions::current(node, town, acting).map_err(|error| error.to_string()))
    }
    pub(crate) fn remember(
        &self,
        node: &Node,
        acting: &Acting,
        town: &TownName,
        source: (Option<crate::ledger::names::Place>, Vec<MachineName>),
        result: &Option<(String, Record)>,
        generation: (u64, u64, u64),
    ) {
        let (place, machines) = source;
        let mut entry = Entry { place, machines, authority: 0, text: hash(&Ok::<_, String>(result)) };
        entry.authority = authority(node, acting, town, &entry);
        if generation != (*node.views.borrow(), *node.ways.borrow(), *node.memory_changes.borrow()) {
            return;
        }
        self.0.lock().expect("never poisoned").insert(key(acting, town), entry);
    }
    pub(super) fn invalidate(&self, town: &TownName) {
        self.0.lock().expect("never poisoned").retain(|(_, name), _| name != town);
    }
    pub(super) fn retain(&self, live: &BTreeSet<Key>) {
        self.0.lock().expect("never poisoned").retain(|key, _| live.contains(key));
    }
}
