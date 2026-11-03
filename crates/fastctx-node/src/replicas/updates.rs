//! One coalescing worker per hub; a changed document never downloads its peers.

use crate::Node;
use fastctx_types::{
    AccountName,
    access::{HubId, TownId},
    account_records::Changed,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct Updates(Mutex<BTreeMap<HubId, Pending>>);
#[derive(Default)]
struct Pending {
    all: bool,
    projects: BTreeSet<TownId>,
    topics: BTreeMap<(TownId, String), u64>,
    records: BTreeMap<(AccountName, String, String), Changed>,
    wake: Arc<tokio::sync::Notify>,
}

pub(crate) enum Change {
    All,
    Project(TownId),
    History { town: TownId, name: String, version: u64 },
    Record(Changed),
}

pub(crate) fn schedule(node: &Arc<Node>, hub: &HubId, change: Change) {
    let wake = {
        let mut updates = node.replicas.updates.0.lock().expect("never poisoned");
        let running = updates.contains_key(hub);
        let pending = updates.entry(hub.clone()).or_default();
        match change {
            Change::All => pending.all = true,
            Change::Project(project) => {
                pending.projects.insert(project);
            }
            Change::History { town, name, version } => {
                pending.topics.entry((town, name)).and_modify(|kept| *kept = (*kept).max(version)).or_insert(version);
            }
            Change::Record(change) => {
                pending.records.insert(
                    (change.account.clone(), change.metadata.kind.clone(), change.metadata.key.clone()),
                    change,
                );
            }
        }
        pending.wake.notify_one();
        if running {
            return;
        }
        pending.wake.clone()
    };
    let (node, hub) = (node.clone(), hub.clone());
    fastctx_platform::task::spawn(async move { run(node, hub, wake).await });
}

async fn run(node: Arc<Node>, hub: HubId, wake: Arc<tokio::sync::Notify>) {
    let mut again = crate::again::Again::default();
    let mut previous = None;
    loop {
        let Some(link) = node.link_of(&hub) else {
            node.replicas.updates.0.lock().expect("never poisoned").remove(&hub);
            return;
        };
        let (all, projects, topics, records) = {
            let mut updates = node.replicas.updates.0.lock().expect("never poisoned");
            let pending = updates.get_mut(&hub).expect("one worker owns the pending entry");
            if !pending.all && pending.projects.is_empty() && pending.topics.is_empty() && pending.records.is_empty() {
                updates.remove(&hub);
                return;
            }
            (
                std::mem::take(&mut pending.all),
                std::mem::take(&mut pending.projects),
                std::mem::take(&mut pending.topics),
                std::mem::take(&mut pending.records),
            )
        };
        let result = async {
            if all {
                super::sync::all(&node, &link).await?;
            } else {
                for project in &projects {
                    super::sync::project(&node, &link, project).await?;
                }
                for ((town, name), version) in &topics {
                    super::sync::topic(&node, &link, town, name, *version).await?;
                }
                for record in records.values() {
                    super::sync::record(&node, &link, record).await?;
                }
            }
            Ok::<_, String>(())
        }
        .await;
        match result {
            Ok(()) => {
                previous = None;
                again = crate::again::Again::default();
            }
            Err(error) => {
                if previous.as_ref() != Some(&error) {
                    node.log.line(format_args!("synchronizing document replicas: {error}"));
                    previous = Some(error);
                }
                {
                    let mut updates = node.replicas.updates.0.lock().expect("never poisoned");
                    let pending = updates.get_mut(&hub).expect("worker entry");
                    pending.all |= all;
                    pending.projects.extend(projects);
                    for (key, version) in topics {
                        pending.topics.entry(key).and_modify(|kept| *kept = (*kept).max(version)).or_insert(version);
                    }
                    for (key, old) in records {
                        pending.records.entry(key).or_insert(old);
                    }
                }
                let mut ways = node.ways.subscribe();
                tokio::select! { () = tokio::time::sleep(again.next()) => {}, () = wake.notified() => {}, _ = ways.changed() => {} }
            }
        }
    }
}
