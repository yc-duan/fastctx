//! One background synchronizer per hub; invalidations coalesce while it works.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use fastctx_types::access::{HubId, TownId};
use tokio::sync::Notify;

use crate::Node;

#[derive(Default)]
pub(crate) struct Updates(Mutex<HashMap<HubId, Pending>>);

#[derive(Default)]
struct Pending {
    all: bool,
    towns: HashSet<TownId>,
    wake: Arc<Notify>,
}

pub(crate) fn changed(node: &Arc<Node>, hub: &HubId, town: Option<TownId>) {
    let wake = {
        let mut updates = node.ledger_updates.0.lock().expect("never poisoned");
        let running = updates.contains_key(hub);
        let pending = updates.entry(hub.clone()).or_default();
        if let Some(town) = town {
            pending.towns.insert(town);
        } else {
            pending.all = true;
        }
        pending.wake.notify_one();
        if running {
            return;
        }
        pending.wake.clone()
    };
    let (node, hub) = (node.clone(), hub.clone());
    tokio::spawn(async move { run(node, hub, wake).await });
}

async fn run(node: Arc<Node>, hub: HubId, wake: Arc<Notify>) {
    let _doing = fastctx_platform::doing::doing(format!("synchronizing ledgers from {hub}"));
    let mut again = crate::again::Again::default();
    let mut previous = None;
    loop {
        let (all, towns) = {
            let mut updates = node.ledger_updates.0.lock().expect("never poisoned");
            let pending = updates.get_mut(&hub).expect("one worker owns this entry");
            if !pending.all && pending.towns.is_empty() {
                updates.remove(&hub);
                return;
            }
            (std::mem::take(&mut pending.all), std::mem::take(&mut pending.towns))
        };
        let mut errors = Vec::new();
        if all {
            errors.extend(super::carry(&node, &hub).await);
            crate::own_collab::carry(&node, &hub).await;
            if let Err(error) = super::sync(&node, &hub, None).await {
                errors.push(error);
            }
        } else {
            for town in &towns {
                if let Err(error) = super::sync(&node, &hub, Some(town.clone())).await {
                    errors.push(error);
                }
            }
        }
        node.shown_changed();
        if errors.is_empty() {
            again = crate::again::Again::default();
            previous = None;
            continue;
        }
        let error = errors.join("; ");
        if previous.as_ref() != Some(&error) {
            node.log.line(format_args!("taking the projects' ledgers: {error}; retrying when connected"));
            previous = Some(error);
        }
        {
            let mut updates = node.ledger_updates.0.lock().expect("never poisoned");
            let pending = updates.get_mut(&hub).expect("one worker owns this entry");
            pending.all |= all;
            pending.towns.extend(towns);
        }
        let mut ways = node.ways.subscribe();
        tokio::select! {
            () = tokio::time::sleep(again.next()) => {},
            () = wake.notified() => {},
            _ = ways.changed() => {},
        }
        // Leaving the network ends this worker; reconnects of a retained link
        // wake it and request a full pass through the same pending entry.
        if node.link_of(&hub).is_none() {
            node.ledger_updates.0.lock().expect("never poisoned").remove(&hub);
            return;
        }
    }
}
