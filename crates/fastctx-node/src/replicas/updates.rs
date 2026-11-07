//! One coalescing worker per hub; a changed document never downloads its peers.

use crate::Node;
use fastctx_types::{
    AccountName,
    access::{HubAccount, HubId, TownId},
    account_records::Changed,
    replicas::Head,
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
    /// Documents a full synchronization could not bring up to date, by copy.
    documents: BTreeMap<String, (HubAccount, Head)>,
    wake: Arc<tokio::sync::Notify>,
}

pub(crate) enum Change {
    All,
    Project(TownId),
    History { town: TownId, name: String, version: u64 },
    Record(Changed),
}

/// Gives the documents a synchronization of `account` could not bring up to
/// date to this hub's worker, to try again on their own. What the first
/// failure was, if any.
pub(crate) fn retry_later(node: &Arc<Node>, account: &HubAccount, failed: Vec<(Head, String)>) -> Option<String> {
    let (_, first) = failed.first()?;
    let error = format!("{} documents not synchronized; the first: {first}", failed.len());
    start(node, &account.hub, false, |pending| {
        for (head, _) in failed {
            pending.documents.insert(super::state::identity(account, &head), (account.clone(), head));
        }
    });
    Some(error)
}

pub(crate) fn schedule(node: &Arc<Node>, hub: &HubId, change: Change) {
    start(node, hub, true, |pending| match change {
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
    });
}

/// Adds to the hub's pending work, starting its worker when none runs. A
/// running worker is woken only for news, not for work given back to it.
fn start(node: &Arc<Node>, hub: &HubId, news: bool, add: impl FnOnce(&mut Pending)) {
    let wake = {
        let mut updates = node.replicas.updates.0.lock().expect("never poisoned");
        let running = updates.contains_key(hub);
        let pending = updates.entry(hub.clone()).or_default();
        add(pending);
        if running {
            if news {
                pending.wake.notify_one();
            }
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
        let (all, projects, topics, records, documents) = {
            let mut updates = node.replicas.updates.0.lock().expect("never poisoned");
            let pending = updates.get_mut(&hub).expect("one worker owns the pending entry");
            if !pending.all
                && pending.projects.is_empty()
                && pending.topics.is_empty()
                && pending.records.is_empty()
                && pending.documents.is_empty()
            {
                updates.remove(&hub);
                return;
            }
            (
                std::mem::take(&mut pending.all),
                std::mem::take(&mut pending.projects),
                std::mem::take(&mut pending.topics),
                std::mem::take(&mut pending.records),
                std::mem::take(&mut pending.documents),
            )
        };
        let given = documents.clone();
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
            let mut failed: BTreeMap<String, (HubAccount, Vec<(Head, String)>)> = BTreeMap::new();
            for (account, head) in documents.into_values() {
                if let Err(error) = super::sync::apply(&node, &link, &account, head.clone()).await {
                    let key = format!("{}/{}", account.hub, account.account);
                    failed.entry(key).or_insert_with(|| (account, Vec::new())).1.push((head, error));
                }
            }
            let mut first = None;
            for (account, failed) in failed.into_values() {
                first = first.or(retry_later(&node, &account, failed));
            }
            first.map_or(Ok(()), Err)
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
                    for (key, document) in given {
                        pending.documents.entry(key).or_insert(document);
                    }
                }
                let mut ways = node.ways.subscribe();
                tokio::select! { () = tokio::time::sleep(again.next()) => {}, () = wake.notified() => {}, _ = ways.changed() => {} }
            }
        }
    }
}
